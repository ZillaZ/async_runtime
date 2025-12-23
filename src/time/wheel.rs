use crate::reactor::Index;
use std::{task::{Context, Poll as StdPoll}, sync::Arc, pin::Pin};
use crate::runtime::{TIMER, SLAB, POLL, Waker};

pub struct Sleep {
    duration: std::time::Duration,
    index: Option<Index>
}

impl Sleep {
    fn setup_poll(&mut self) {
        TIMER.with(|timer| {
            let mut timer = timer.borrow_mut();
            unsafe {
                timer.add_duration(self.duration, std::mem::transmute(self.index.unwrap()));
            }
        });
    }
}

impl Future for Sleep {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if self.index.is_some() {
            return StdPoll::Ready(().into());
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        return StdPoll::Pending
    }
}

impl Drop for Sleep {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}

use std::collections::BinaryHeap;

#[derive(Clone, Copy)]
struct TimerDuration {
    secs: i64,
    nanos: i64,
    index: Index,
    timespec: libc::timespec
}

impl PartialOrd for TimerDuration {
    fn ge(&self, other: &Self) -> bool {
        let (secs, nanos) = ((self.secs - other.secs), (self.nanos - other.nanos));
        secs >= 0 || (secs == 0 && nanos >= 0)
    }
    fn gt(&self, other: &Self) -> bool {
        let (secs, nanos) = ((self.secs - other.secs), (self.nanos - other.nanos));
        secs > 0 || (secs == 0 && nanos > 0)
    }
    fn le(&self, other: &Self) -> bool {
        let (secs, nanos) = ((self.secs - other.secs), (self.nanos - other.nanos));
        secs <= 0 || (secs == 0 && nanos <= 0)
    }
    fn lt(&self, other: &Self) -> bool {
        let (secs, nanos) = ((self.secs - other.secs), (self.nanos - other.nanos));
        secs < 0 || (secs == 0 && nanos < 0)
    }
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self.gt(other) {
            Some(std::cmp::Ordering::Greater)
        }else if self.lt(other){
            Some(std::cmp::Ordering::Less)
        }else if self.eq(other) {
            Some(std::cmp::Ordering::Equal)
        }else{
            None
        }
    }
}

impl Eq for TimerDuration {}

impl PartialEq for TimerDuration {
    fn eq(&self, other: &Self) -> bool {
        self.secs == other.secs && self.nanos == other.nanos
    }

    fn ne(&self, other: &Self) -> bool {
        !self.eq(other)
    }
}

impl TimerDuration {
    fn new(secs: i64, nanos: i64, index: Index) -> Self {
        let timespec = libc::timespec { tv_sec: secs, tv_nsec: nanos };
        TimerDuration { secs, nanos, index, timespec }
    }
}

impl Ord for TimerDuration {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if (self.secs, self.nanos) == (other.secs, other.nanos) {
            return std::cmp::Ordering::Equal;
        }
        let sub = ((self.secs - other.secs), (self.nanos - other.nanos));
        if sub.0 <= 0 && sub.1 < 0 {
            std::cmp::Ordering::Greater
        }else{
            std::cmp::Ordering::Less
        }
    }
}

enum TimerState {
    Idle,
    Waiting,
    Canceling,
}

pub struct Timer {
    fd: i32,
    heap: BinaryHeap<TimerDuration>,
    last: Option<TimerDuration>,
    index: Index,
    state: TimerState
}

impl Timer {
    pub fn new() -> Self {
        let fd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC) };
        let index = SLAB.with(|slab| {
            let waker = std::task::Waker::from(Arc::new(Waker::new(0)));
            slab.borrow_mut().add(waker)
        });
        Self { fd, index, heap: BinaryHeap::new(), last: None, state: TimerState::Idle }
    }

    pub fn is_timer(&self, index: u64) -> bool {
        self.index.index as u64 == index
    }

    fn cancel_timer(&mut self) {
        let Some(last) = self.last else {
            return;
        };
        self.state = TimerState::Canceling;
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).addr_u.addr = std::mem::transmute(self.index);
                (*sqe).off_u.addr2 = std::mem::transmute(&last.timespec);
                (*sqe).opcode = uring_lib::IORING_OP_TIMEOUT_REMOVE;
                (*sqe).user_data = std::mem::transmute(self.index);
            }
            poll.register();
        });
    }

    fn next_timer(&mut self) {
        let Some(timer_duration) = self.heap.pop() else {
            self.state = TimerState::Idle;
            self.last = None;
            return;
        };
        self.last = Some(timer_duration);
        self.setup_timer();
    }

    fn setup_timer(&mut self) {
        let Some(timer_duration) = self.last else {
            return;
        };
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).fd = self.fd;
                (*sqe).opcode = uring_lib::IORING_OP_TIMEOUT;
                (*sqe).len = 1;
                (*sqe).user_data = std::mem::transmute(self.index);
                (*sqe).addr_u.addr = std::mem::transmute(&timer_duration.timespec);
                (*sqe).oflags.timeout_flags = uring_lib::IORING_TIMEOUT_ABS;
            }
            poll.register();
            self.state = TimerState::Waiting;
        });
    }

    fn handle_cancel(&mut self, res: i32) {
        if res != -libc::ECANCELED && res != -libc::ETIME { return }
        self.setup_timer();
    }

    fn wakeup(&mut self, res: i32) {
        if res != -libc::ETIME { return }
        let Some(timer_duration) = self.last else {
            return;
        };
        SLAB.with(|slab| {
            let slab = slab.borrow();
            let waker = slab.get_waker(timer_duration.index).unwrap();
            waker.wake_by_ref();
        });
        self.next_timer();
    }

    pub fn handle_event(&mut self, res: i32) {
        match self.state {
            TimerState::Idle => return,
            TimerState::Canceling => self.handle_cancel(res),
            TimerState::Waiting => self.wakeup(res),
        }
    }

    pub fn add_duration(&mut self, duration: std::time::Duration, index: Index) {
        let (secs, nanos) = duration_to_secs_nanos(duration);
        let mut timespec = libc::timespec { tv_sec: secs, tv_nsec: nanos };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, std::mem::transmute(&mut timespec)); }
        let (secs, nanos) = (secs + timespec.tv_sec, nanos + timespec.tv_nsec);
        let timer_duration = TimerDuration::new(secs, nanos, index);
        if let Some(last) = self.last {
            if timer_duration < last {
                self.cancel_timer();
                self.heap.push(last);
                self.last = Some(timer_duration);
            }else{
                self.heap.push(timer_duration);
            }
        }else {
            self.last = Some(timer_duration);
            self.setup_timer();
        }
    }
}

fn duration_to_secs_nanos(duration: std::time::Duration) -> (i64, i64) {
    let secs_as_nanos = (duration.as_secs() as u128) * (10e8 as u128);
    let nanos = (duration.as_nanos() - secs_as_nanos) as i64;
    (duration.as_secs() as i64, nanos)
}

pub fn sleep(duration: std::time::Duration) -> Sleep {
    Sleep { duration, index: None }
}
