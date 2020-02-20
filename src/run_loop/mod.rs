
pub mod task;

use crate::binary_heap::{BinaryHeap, Node as BHNode};
use crate::linked_list::List;

use std::sync::{mpsc, Arc};
use std::time::Instant;
use std::cell::{UnsafeCell, Cell};
use std::thread;
use std::future::Future;

trait TimedAction {
    fn update(&mut self, time: Instant) -> Option<Instant>;
}

pub fn status() -> Status { RUN_LOOP.with(|t| t.status.get() )}
pub fn run() { RUN_LOOP.with(|t| t.run() )}
pub fn tick() -> bool { RUN_LOOP.with(|t| t.tick() )}
pub fn post_quit() {
    RUN_LOOP.with(|t| {
        if t.status.get() == Status::Running {
            t.status.set(Status::Quiting);
            t.tx.send(Msg::Quit).unwrap();
        }
    })
}
pub fn post<F: FnOnce() + Send + 'static>(msg: F) {
    RUN_LOOP.with(|t| { t.tx.send(Msg::Func(Box::new(msg))).unwrap() })
}
pub fn spawn_task<'a, T: Future + 'a, F: FnOnce() -> T>(task: F) -> task::Task<'a, T::Output> { task::Task::new(task()) }


#[derive(Clone)]
pub struct Handle {
    tx: mpsc::Sender<Msg>,
    thread_id: thread::ThreadId,
}

impl Handle {
    pub fn new() -> Handle {
        Handle {
            tx: RUN_LOOP.with(|t| t.tx.clone()),
            thread_id: thread::current().id(),
        }
    }
    pub fn is_local(&self) -> bool { thread::current().id() == self.thread_id }
    pub fn post<F: FnOnce() + Send + 'static>(&self, msg: F) -> Result<(), mpsc::SendError<Box<dyn FnOnce() + Send + 'static>>> {
        self.tx.send(Msg::Func(Box::new(msg)))
            .map_err(|e| match e.0 {
                Msg::Func(t) => mpsc::SendError(t),
                _ => unreachable!(),
            })
    }
    pub fn post_quit(&self) -> Result<(), ()> {
        self.post(|| post_quit()).map_err(|_| ())
    }
}

impl Default for Handle {
    fn default() -> Handle {
        Handle::new()
    }
}

enum Msg {
    Quit,
    Func(Box<dyn FnOnce() + Send + 'static>),
}

#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub enum Status {
    Quit,
    Running,
    Quiting,
}

struct RunLoop {
    status: Cell<Status>,
    tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    deadlines: UnsafeCell<BinaryHeap<Instant, dyn TimedAction>>,
    waken_tasks: UnsafeCell<List<dyn task::Executor>>,
    pending_tasks: UnsafeCell<List<dyn task::Executor>>,
}

impl Default for RunLoop {
    fn default() -> RunLoop {
        let (tx, rx) = mpsc::channel();
        RunLoop {
            status: Cell::new(Status::Quit),
            tx, rx,
            deadlines: UnsafeCell::default(),
            waken_tasks: UnsafeCell::default(),
            pending_tasks: UnsafeCell::default(),
        }
    }
}

impl RunLoop {
    fn run(&self) {
        let _guard = RunningGuard::new(self);
        'main: loop {
            while let Ok(msg) = self.rx.try_recv() {
                if !self.process_msg(msg) {
                    break 'main;
                }
            }

            loop {
                let deadline = self.process_deadlines();

                if self.process_tasks() {
                    continue;
                }

                let msg = if let Some(deadline) = deadline {
                    match self.rx.recv_deadline(deadline) {
                        Ok(msg) => msg,
                        Err(_) => continue,
                    }
                } else {
                    self.rx.recv().unwrap()
                };

                if !self.process_msg(msg) {
                    break 'main;
                }
            }
        }
    }

    pub fn tick(&self) -> bool {
        let _guard = RunningGuard::new(self);

        while let Ok(msg) = self.rx.try_recv() {
            if !self.process_msg(msg) {
                return false;
            }
        }

        self.process_deadlines();

        self.process_tasks();

        while let Ok(msg) = self.rx.try_recv() {
            if !self.process_msg(msg) {
                return false;
            }
        }

        true
    }

    fn process_msg(&self, msg: Msg) -> bool {
        match msg {
            Msg::Quit => return false,
            Msg::Func(func) => func(),
        }
        true
    }

    fn process_deadlines(&self) -> Option<Instant> {
        unsafe {
            let deadlines = &mut *self.deadlines.get();
            let now = Instant::now();
            while let Some(&item) = deadlines.peek() {
                let mut item = item;
                let key = *BHNode::key(item.as_ref());
                if key <= now {
                    if let Some(t) = item.as_mut().update(key) {
                        deadlines.update(item, t);
                    } else {
                        deadlines.remove(BHNode::index(item.as_ref()));
                    }
                } else {
                    return Some(*BHNode::key(item.as_mut()));
                }
            }
            None
        }
    }

    fn process_tasks(&self) -> bool {
        let mut processed = false;

        loop {
            unsafe {
                let node = (*self.waken_tasks.get()).pop_front();
                if node.is_null() {
                    break;
                }
                processed = true;
                let node = Arc::from_raw(node);
                if node.exec(&node).is_pending() {
                    let list = self.pending_tasks.get();
                    *node.list_mut() = list;
                    (*list).push_back(Arc::into_raw_non_null(node));
                } else {
                    *node.list_mut() = 0 as *mut _;
                }
            }
        }

        processed
    }
}

impl Drop for RunLoop {
    fn drop(&mut self) {
        unsafe {
            (*self.waken_tasks.get()).for_each(|node| node.as_ref().run_loop_drop());
            (*self.pending_tasks.get()).for_each(|node| node.as_ref().run_loop_drop());

            (*self.waken_tasks.get()).clear(|node| {
                Arc::from_raw(node.as_ptr());
            });
            (*self.pending_tasks.get()).clear(|node| {
                Arc::from_raw(node.as_ptr());
            });
        }
    }
}

struct RunningGuard<'a> {
    rl: &'a RunLoop,
}

impl<'a> RunningGuard<'a> {
    fn new(rl: &'a RunLoop) -> RunningGuard<'a> {
        assert_eq!(rl.status.get(), Status::Quit);
        rl.status.set(Status::Running);
        RunningGuard { rl }
    }
}

impl<'a> Drop for RunningGuard<'a> {
    fn drop(&mut self) {
        self.rl.status.set(Status::Quit);
    }
}

thread_local! {
    static RUN_LOOP: RunLoop = RunLoop::default();
}

/*
unsafe fn deadlines<T, F: FnOnce(&mut BinaryHeap<Instant, dyn TimedAction>) -> T>(f: F) -> T {
    RUN_LOOP.with(|t| f(&mut *t.deadlines.get()))
}

unsafe fn try_deadlines<T, F: FnOnce(&mut BinaryHeap<Instant, dyn TimedAction>) -> T>(f: F) -> Option<T> {
    RUN_LOOP.try_with(|t| f(&mut *t.deadlines.get())).ok()
}
*/