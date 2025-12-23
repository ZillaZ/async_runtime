use std::{pin::Pin, rc::Rc, cell::RefCell, sync::{Arc, LazyLock}, collections::HashMap, task::{Context, Wake, Poll as StdPoll}};
use crate::reactor::{Poll, Slab, TokenManager};
use crate::time::Timer;

thread_local! {
    pub static TASK_QUEUE : LazyLock<Rc<RefCell<Vec<(u64, AsyncTask)>>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Vec::new()))
    });
    pub static COMPLETION_QUEUE : LazyLock<Rc<RefCell<Vec<u64>>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Vec::new()))
    });
    pub static SLAB : LazyLock<Rc<RefCell<Slab>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Slab::new()))
    });
    pub static POLL : LazyLock<Rc<RefCell<Poll>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Poll::new()))
    });
    pub static TASKS : std::sync::LazyLock<Rc<RefCell<HashMap<u64, AsyncTask>>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(HashMap::new()))
    });
    pub static TOKEN_MANAGER : LazyLock<Rc<RefCell<TokenManager>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(TokenManager::new()))
    });

    pub static TIMER : LazyLock<Rc<RefCell<Timer>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Timer::new()))
    });
}

type AsyncTask = Pin<Box<dyn Future<Output = ()> + Send + Sync>>;

pub struct NoirRuntime<'a> {
    context: Context<'a>
}

impl <'a> NoirRuntime<'a> {
    pub fn new(waker: &'a std::task::Waker) -> Self {
        Self { context: Context::from_waker(waker) }
    }

    pub fn work(&mut self) {
        let r = self.exec();
        r.iter().for_each(|x| {
            TASKS.with(|tasks| {
                let mut tasks = tasks.borrow_mut();
                tasks.remove(x);
            });
        });
        loop {
            self.drain_queue();
            self.drain_completion();
            let Ok((ret, idx)) = POLL.with(|poll| {
                poll.borrow_mut().poll()
            }) else {
                continue
            };
            let is_timer = TIMER.with(|timer| {
                let mut timer = timer.borrow_mut();
                if !timer.is_timer(idx) { return false };
                timer.handle_event(ret);
                true
            });
            if is_timer {
                continue;
            }
            SLAB.with(|slab| {
                let idx = unsafe { std::mem::transmute(idx) };
                let mut slab = slab.borrow_mut();
                slab.add_result(idx, ret.into());
                let waker = slab.get_waker(idx).unwrap();
                waker.wake_by_ref();
            });
        }
    }

    fn exec_one(&self, task: &mut AsyncTask, waker: &std::task::Waker) -> bool {
        let mut context = Context::from_waker(waker);
        task.as_mut().poll(&mut context).is_ready()
    }

    fn drain_completion(&mut self) {
        COMPLETION_QUEUE.with(|queue| {
            let mut queue = queue.borrow_mut();
            while let Some(token) = queue.pop() {
                TASKS.with(|tasks| {
                    let mut tasks = tasks.borrow_mut();
                    let finished = {
                        let Some(task) = tasks.get_mut(&token) else {
                            return;
                        };
                        let waker = SLAB.with(|slab| {
                            let slab = slab.borrow();
                            slab.get_waker_by_token(token).unwrap().clone()
                        });
                        self.exec_one(task, &waker)
                    };
                    if finished {
                        tasks.remove(&token);
                        SLAB.with(|slab| {
                            slab.borrow_mut().clear_token(token);
                        });
                    }
                });
            }
        });
    }

    fn drain_queue(&mut self) {
        TASKS.with(|tasks| {
            let mut tasks = tasks.borrow_mut();
            TASK_QUEUE.with(|queue| {
                let mut queue = queue.borrow_mut();
                while let Some((token, mut task)) = queue.pop() {
                    let waker = std::task::Waker::from(Arc::new(Waker::new(token)));
                    let mut context = Context::from_waker(&waker);
                    match task.as_mut().poll(&mut context) {
                        StdPoll::Pending => {
                            SLAB.with(|slab| {
                                slab.borrow_mut().add_by_token(token, waker.clone());
                            });
                            tasks.insert(token, task);
                        },
                        StdPoll::Ready(_) => ()
                    };
                }
            });
        });
    }

    pub fn exec(&mut self) -> Vec<u64> {
        let mut to_remove = vec![];
        TASKS.with(|tasks| {
            let mut tasks = tasks.borrow_mut();
            for (token, task) in tasks.iter_mut() {
                match task.as_mut().poll(&mut self.context) {
                    StdPoll::Pending => (),
                    StdPoll::Ready(_) => {
                        to_remove.push(*token);
                    }
                }
            }
            TASK_QUEUE.with(|queue| {
                let mut queue = queue.borrow_mut();
                while let Some((token, mut task)) = queue.pop() {
                    let waker = std::task::Waker::from(Arc::new(Waker::new(token)));
                    SLAB.with(|slab| {
                        slab.borrow_mut().add_by_token(token, waker.clone());
                    });
                    let mut context = Context::from_waker(&waker);
                    match task.as_mut().poll(&mut context) {
                        StdPoll::Pending => {
                            tasks.insert(token, task);
                        },
                        StdPoll::Ready(_) => ()
                    };
                }
            });
        });
        to_remove
    }
}

#[derive(Clone)]
pub struct Waker {
    token: u64
}

impl Waker {
    pub fn new(token: u64) -> Self {
        Self { token }
    }
}

impl Wake for Waker {
    fn wake(self: std::sync::Arc<Self>) {
        COMPLETION_QUEUE.with(|queue| {
            queue.borrow_mut().push(self.token);
        });
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        COMPLETION_QUEUE.with(|queue| {
            queue.borrow_mut().push(self.token);
        });
    }
}

impl Drop for Waker {
    fn drop(&mut self) {
        println!("DROPPING WAKER {}", self.token);
        let _ = TOKEN_MANAGER.try_with(|manager| {
            manager.borrow_mut().clear_token(self.token);
        });
    }
}

pub fn initialize_runtime() {
    let waker = Waker::new(0);
    let waker = std::task::Waker::from(Arc::new(waker));
    SLAB.with(|slab| {
        slab.borrow_mut().add_by_token(0, waker.clone());
    });
    let mut runtime = NoirRuntime::new(&waker);
    runtime.work();
}
