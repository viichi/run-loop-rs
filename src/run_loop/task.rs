
use std::future::Future;
use std::task::{Waker, RawWakerVTable, RawWaker, Context, Poll};
use std::num::Wrapping;
use std::sync::{Arc, Weak};
use std::marker::PhantomData;
use std::cell::UnsafeCell;
use std::pin::Pin;

use crate::linked_list::{Node as ListNode, NodePtr as ListNodePtr, List};

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
    unsafe fn wake(&self, ticket: usize) -> bool;
    unsafe fn exec(&self, node: &Arc<ListNode<dyn Executor>>) -> Exec;
    unsafe fn list_mut(&self) -> &mut *mut List<dyn Executor>;
    unsafe fn run_loop_drop(&self);
}

trait TaskBase<T> {
    unsafe fn poll(&self, cx: &mut Context<'_>) -> Poll<T>;
    unsafe fn to_executor_node_ptr(&self) -> ListNodePtr<dyn Executor>;
}

struct RawTask<T: Future> {
    future: T,
    result: TaskResult<T::Output>,
    waker: Option<Waker>,
    ticket: Wrapping<usize>,
    list: *mut List<dyn Executor>,
}

impl<T: Future> Executor for UnsafeCell<RawTask<T>> {
    unsafe fn wake(&self, ticket: usize) -> bool {
        let task = &mut *self.get();
        if task.ticket.0 == ticket {
            task.ticket += Wrapping(1);
            true
        } else {
            false
        }
    }

    unsafe fn exec(&self, node: &Arc<ListNode<dyn Executor>>) -> Exec {
        let task = &mut *self.get();
        let dummy = DummyWaker { node, ticket: task.ticket.0 };
        let waker = Waker::from_raw(dummy.to_raw_waker());
        let fut = Pin::new_unchecked(&mut task.future);
        let mut cx = Context::from_waker(&waker);
        match fut.poll(&mut cx) {
            Poll::Pending => Exec::Pending,
            Poll::Ready(t) => {
                task.ticket += Wrapping(1);
                task.result = TaskResult::Ready(t);
                if let Some(w) = task.waker.take() {
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

    unsafe fn run_loop_drop(&self) {
        let task = &mut *self.get();
        task.ticket += Wrapping(1);
        task.list = 0 as *mut _;
    }
}

impl<T: Future> TaskBase<T::Output> for UnsafeCell<RawTask<T>> {
    unsafe fn poll(&self, cx: &mut Context<'_>) -> Poll<T::Output> {
        let task = &mut *self.get();
        let poll = task.result.poll();
        if poll.is_pending() {
            if let Some(ref mut waker) = task.waker {
                if !cx.waker().will_wake(waker) {
                    *waker = cx.waker().clone();
                }
            } else {
                task.waker = Some(cx.waker().clone());
            }
        }
        poll
    }

    unsafe fn to_executor_node_ptr(&self) -> ListNodePtr<dyn Executor> {
        let ptr = self as *const Self;
        let ptr = ptr as *const dyn Executor;
        let node_ptr = ListNode::get_ptr_from_raw(ptr);
        ListNodePtr::new_unchecked(node_ptr as *mut _)
    }
}

struct DummyWaker<'a> {
    node: &'a Arc<ListNode<dyn Executor>>,
    ticket: usize,
}

impl<'a> DummyWaker<'a> {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone,
        Self::nop,
        Self::nop,
        Self::nop,
    );

    fn to_raw_waker(&self) -> RawWaker {
        RawWaker::new( self as *const Self as *const _, &Self::VTABLE)
    }

    unsafe fn clone(this: *const ()) -> RawWaker {
        let this = &*(this as *const DummyWaker);
        TaskWaker::new_raw_waker(this.node, this.ticket)
    }

    unsafe fn nop(_this: *const ()) {

    }
}

struct TaskWaker {
    handle: super::Handle,
    node: Weak<ListNode<dyn Executor>>,
    ticket: usize,
}

struct TWData (Weak<ListNode<dyn Executor>>, usize);
unsafe impl Send for TWData {}

impl TaskWaker {
    const VTABLE: RawWakerVTable = RawWakerVTable::new(
        Self::clone,
        Self::wake,
        Self::wake_by_ref,
        Self::drop,
    );

    fn new_raw_waker(node: &Arc<ListNode<dyn Executor>>, ticket: usize) -> RawWaker {
        let waker = Box::new(TaskWaker {
            handle: super::Handle::new(),
            node: Arc::downgrade(node),
            ticket,
        });
        RawWaker::new(Box::into_raw(waker) as *const _, &Self::VTABLE)
    }

    unsafe fn clone(this: *const ()) -> RawWaker {
        let waker = &*(this as *const TaskWaker);
        let cloned = Box::new(TaskWaker {
            handle: waker.handle.clone(),
            node: waker.node.clone(),
            ticket: waker.ticket,
        });
        RawWaker::new(Box::into_raw(cloned) as *const _, &Self::VTABLE)
    }

    unsafe fn wake_by_ref(this: *const ()) {
        let waker = &*(this as *const TaskWaker);
        let data = TWData(waker.node.clone(), waker.ticket);
        let _ = waker.handle.post(move || {
            if let Some(node) = data.0.upgrade() {
                node.wake(data.1);
            }
        });
    }

    unsafe fn wake(this: *const ()) {
        let waker = Box::from_raw(this as *mut TaskWaker);

        let data = TWData(waker.node, waker.ticket);
        if waker.handle.is_local() {
            if let Some(node) = data.0.upgrade() {
                if node.wake(data.1) {
                    pending_to_waken(node);
                }
            }
        } else {
            let _ = waker.handle.post(move || {
                if let Some(node) = data.0.upgrade() {
                    if node.wake(data.1) {
                        pending_to_waken(node);
                    }
                }
            });
        }
    }

    unsafe fn drop(this: *const ()) {
        Box::from_raw(this as *mut TaskWaker);
    }
}

pub struct Task<'a, T> {
    node: Arc<ListNode<dyn TaskBase<T>>>,
    phantom: PhantomData<&'a ()>,
}

impl<'a, T> Task<'a, T> {
    pub fn new<F: 'a + Future<Output = T>>(future: F) -> Task<'a, T> {
        let task = Arc::new(ListNode::new(UnsafeCell::new(RawTask {
            future,
            result: TaskResult::Pending,
            waker: None,
            ticket: Wrapping(0),
            list: 0 as *mut _,
        })));

        unsafe {
            std::mem::forget(task.clone());

            let task_ptr = Arc::into_raw(task);
            let base_ptr = task_ptr as *const ListNode<dyn TaskBase<T>>;
            let exec_ptr = task_ptr as *const ListNode<dyn Executor>;
            super::RUN_LOOP.with(|rl| {
                let list = rl.waken_tasks.get();
                (*(*task_ptr).get()).list = list;
                (*list).push_back(ListNodePtr::new_unchecked(exec_ptr as *mut _))
            });

            Task {
                node: Arc::from_raw(base_ptr as *mut ListNode<dyn TaskBase<T>>),
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
            let node_ptr = self.node.to_executor_node_ptr();
            let list = node_ptr.as_ref().list_mut();
            if !list.is_null() {
                (**list).remove(node_ptr);
                *list = 0 as *mut _;
                Arc::from_raw(node_ptr.as_ptr());
            }
        }
    }
}

impl<'a, T> Future for Task<'a, T> {
    type Output = T;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<T> {
        unsafe { self.node.poll(cx) }
    }
}

unsafe fn pending_to_waken(node: Arc<ListNode<dyn Executor>>) {
    super::RUN_LOOP.with(|rl| {
        let pending_list = rl.pending_tasks.get();
        let waken_list = rl.waken_tasks.get();
        let ptr = ListNodePtr::new_unchecked(
            node.as_ref() as *const _ as *mut ListNode<dyn Executor>);
        (*pending_list).remove(ptr);
        (*waken_list).push_back(ptr);
        *node.list_mut() = waken_list;
    })
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