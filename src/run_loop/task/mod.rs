
use std::future::Future;
use std::task::{Waker, RawWakerVTable, RawWaker, Context, Poll};
use std::sync::{Arc, Weak};
use std::marker::PhantomData;
use std::cell::UnsafeCell;
use std::pin::Pin;

use crate::linked_list::{Node as ListNode, NodePtr as ListNodePtr, List};
use crate::spin_lock::SpinLock;
use std::mem::MaybeUninit;

#[derive(Eq, PartialEq, Copy, Clone)]
pub(crate) enum Exec {
    Pending,
    Ready,
}

impl Exec {
    pub(crate) fn is_pending(self) -> bool {
        self == Exec::Pending
    }
}

enum TaskResult<T> {
    Pending,
    Ready(T),
    Polled,
}

impl<T> TaskResult<T> {
    fn poll(&mut self) -> Poll<T> {
        match std::mem::replace(self, TaskResult::Polled) {
            TaskResult::Pending => Poll::Pending,
            TaskResult::Ready(t) => Poll::Ready(t),
            TaskResult::Polled => panic!(),
        }
    }
}

pub(crate) trait Executor {
    unsafe fn wake_by(&self, waker: &Arc<TaskWaker>) -> bool;
    unsafe fn exec(&self) -> Exec;
    unsafe fn list_mut(&self) -> &mut *mut List<dyn Executor>;
}

trait TaskBase<T> {
    unsafe fn poll(&self, cx: &mut Context<'_>) -> Poll<T>;
    unsafe fn task_drop(&self);
}

struct RawTask<T: Future> {
    future: T,
    result: TaskResult<T::Output>,
    notifier: Option<Waker>,
    list: *mut List<dyn Executor>,
    waker: MaybeUninit<Arc<TaskWaker>>,
}

impl<T: Future> RawTask<T> {
    const WAKER: RawWakerVTable = RawWakerVTable::new(
        Self::clone,
        Self::wake,
        Self::wake_by_ref,
        Self::drop,
    );

    fn into_waker(waker: Arc<TaskWaker>) -> Waker {
        let raw = RawWaker::new(Arc::into_raw(waker) as *const _ as *const _, &Self::WAKER);
        unsafe { Waker::from_raw(raw) }
    }

    unsafe fn clone(this: *const ()) -> RawWaker {
        let waker = Arc::from_raw(this as *const TaskWaker);
        std::mem::forget(waker.clone());
        RawWaker::new(Arc::into_raw(waker) as *const _, &Self::WAKER)
    }

    unsafe fn wake(this: *const ()) {
        let waker = Arc::from_raw(this as *const TaskWaker);
        let handle = waker.handle.lock();
        if handle.is_local() {
            drop(handle);
            if let Some(node) = waker.node.upgrade() {
                Self::try_wake(&waker, node);
            }
        } else {
            let cloned_handle = handle.clone();
            drop(handle);
            struct Data(Arc<TaskWaker>);
            unsafe impl Send for Data {};
            let data = Data(waker);
            let _ = cloned_handle.post(move || {
                if let Some(node) = data.0.node.upgrade() {
                    Self::try_wake(&data.0, node);
                }
            });
        }
    }

    unsafe fn wake_by_ref(this: *const ()) {
        let waker = Arc::from_raw(this as *const TaskWaker);
        let handle = waker.handle.lock();
        if handle.is_local() {
            drop(handle);
            if let Some(node) = waker.node.upgrade() {
                Self::try_wake(&waker, node);
            }
        } else {
            struct Data(Arc<TaskWaker>);
            unsafe impl Send for Data {};
            let data = Data(waker.clone());
            let _ = handle.post(move || {
                if let Some(node) = data.0.node.upgrade() {
                    Self::try_wake(&data.0, node);
                }
            });
            drop(handle);
        }
        std::mem::forget(waker);
    }

    unsafe fn try_wake(waker: &Arc<TaskWaker>, node: Arc<ListNode<dyn Executor>>) {
        super::RUN_LOOP.with(|rl| {
            if rl.pending_tasks.get() == *node.list_mut() && node.wake_by(waker) {
                (*rl.pending_tasks.get()).remove(ListNodePtr::new_unchecked(node.as_ref() as *const _ as *mut _));
                (*rl.waken_tasks.get()).push_back(ListNodePtr::new_unchecked(node.as_ref() as *const _ as *mut _));
            }
        });
    }

    unsafe fn drop(this: *const ()) {
        Arc::from_raw(this as *const TaskWaker);
    }
}

impl<T: Future> Drop for RawTask<T> {
    fn drop(&mut self) {
        unsafe { self.waker.as_ptr().read(); }
    }
}

impl<T: Future> Executor for UnsafeCell<RawTask<T>> {
    unsafe fn wake_by(&self, waker: &Arc<TaskWaker>) -> bool {
        let task = &mut *self.get();
        if Arc::ptr_eq(&task.waker.get_ref(), &waker) {
            *task.waker.get_mut() = Arc::new(TaskWaker {
                handle: Default::default(),
                node: task.waker.get_ref().node.clone(),
            });
            true
        } else {
            false
        }
    }

    unsafe fn exec(&self) -> Exec {
        let task = &mut *self.get();
        let waker = RawTask::<T>::into_waker(task.waker.get_ref().clone());
        let fut = Pin::new_unchecked(&mut task.future);
        let mut cx = Context::from_waker(&waker);
        match fut.poll(&mut cx) {
            Poll::Pending => Exec::Pending,
            Poll::Ready(t) => {
                task.result = TaskResult::Ready(t);
                if let Some(w) = task.notifier.take() {
                    w.wake();
                }
                Exec::Ready
            },
        }
    }

    unsafe fn list_mut(&self) -> &mut *mut List<dyn Executor> {
        let task = self.get();
        &mut (*task).list
    }
}

impl<T: Future> TaskBase<T::Output> for UnsafeCell<RawTask<T>> {
    unsafe fn poll(&self, cx: &mut Context<'_>) -> Poll<T::Output> {
        let task = &mut *self.get();
        let poll = task.result.poll();
        if poll.is_pending() {
            if let Some(ref mut waker) = task.notifier {
                if !cx.waker().will_wake(waker) {
                    *waker = cx.waker().clone();
                }
            } else {
                task.notifier = Some(cx.waker().clone());
            }
        }
        poll
    }

    unsafe fn task_drop(&self) {
        let task = &mut *self.get();
        if !task.list.is_null() {
            let ptr = self as *const _ as *const dyn Executor;
            let ptr = ListNode::get_ptr_from_raw(ptr);
            (*task.list).remove(ListNodePtr::new_unchecked(ptr as *mut _));
            task.list = 0 as *mut _;
            Arc::from_raw(ptr);
        }
    }
}

pub(crate) struct TaskWaker {
    handle: SpinLock<super::Handle>,
    node: Weak<ListNode<dyn Executor>>,
}

pub struct Task<'a, T> {
    node: Arc<ListNode<dyn TaskBase<T>>>,
    phantom: PhantomData<&'a ()>,
}

impl<'a, T> Task<'a, T> {
    pub fn new<F: 'a + Future<Output = T>>(future: F) -> Task<'a, T> {
        unsafe {
            let task = Arc::new(ListNode::new(UnsafeCell::new(RawTask {
                future,
                result: TaskResult::Pending,
                notifier: None,
                list: 0 as *mut _,
                waker: MaybeUninit::uninit(),
            })));
            std::mem::forget(task.clone());

            let node_ptr = Arc::into_raw(task);
            let base_ptr = node_ptr as *const ListNode<dyn TaskBase<T>>;
            let exec_ptr = node_ptr as *const ListNode<dyn Executor> as *mut _;

            let raw_task_mut = &mut *(*node_ptr).as_ref().get();

            super::RUN_LOOP.with(|rl| {
                let list = rl.waken_tasks.get();
                raw_task_mut.list = list;
                (*list).push_back(ListNodePtr::new_unchecked(exec_ptr))
            });

            let exec = Arc::from_raw(exec_ptr);
            raw_task_mut.waker.as_mut_ptr().write(Arc::new(TaskWaker {
                handle: Default::default(),
                node: Arc::downgrade(&exec),
            }));
            std::mem::forget(exec);

            let node = Arc::from_raw(base_ptr as *mut _);

            assert_eq!(Arc::strong_count(&node), 2);

            Task {
                node,
                phantom: PhantomData,
            }
        }
    }
}

impl<T> Task<'static, T> {
    pub fn detach(self) {
        unsafe {
            std::ptr::read(&self.node);
        }
        std::mem::forget(self);
    }
}

impl<'a, T> Drop for Task<'a, T> {
    fn drop(&mut self) {
        unsafe {
            self.node.task_drop();
        }
    }
}

impl<'a, T> Future for Task<'a, T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        unsafe { self.node.poll(cx) }
    }
}

pub struct Yield (bool);

impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            let waker = cx.waker().clone();
            super::post(move || waker.wake());
            Poll::Pending
        }
    }
}

pub fn yield_now() -> Yield {
    Yield (false)
}