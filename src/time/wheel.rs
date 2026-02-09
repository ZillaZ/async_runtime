use io_uring::{opcode::Timeout as ITimeout, squeue::Flags, types::{TimeoutFlags, Timespec}};

use crate::reactor::Index;
use std::{task::{Context, Poll as StdPoll}, sync::Arc, pin::Pin};
use crate::runtime::{SLAB, POLL, Waker};
use crate::io_uring::api::{Timeout, LinkedTimeout};

use std::collections::BinaryHeap;
use std::cmp::Reverse;

#[derive(Clone, Copy, Debug)]
struct TimerDuration {
    secs: i64,
    nanos: i64,
    task_index: Index,
    timer_index: Index,
    flags: Option<Flags>,
    linked: bool
}

impl TimerDuration {
    fn new(secs: i64, nanos: i64, task_index: Index, timer_index: Index, flags: Option<Flags>, linked: bool) -> Self {
        TimerDuration { secs, nanos, task_index, timer_index, flags, linked }
    }

    fn to_timespec(&self) -> Timespec {
        Timespec::new().sec(self.secs as u64).nsec(self.nanos as u32)
    }

    // Create from current time + duration
    fn from_duration(duration: std::time::Duration, task_index: Index, timer_index: Index, flags: Option<Flags>, linked: bool) -> Self {
        let mut now = libc::timespec { tv_sec: 0, tv_nsec: 0 };
        unsafe {
            libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut now);
        }

        let duration_secs = duration.as_secs() as i64;
        let duration_nanos = (duration.as_nanos() % 1_000_000_000) as i64;

        // Add duration to current time with proper carry handling
        let mut total_nanos = now.tv_nsec + duration_nanos;
        let mut total_secs = now.tv_sec + duration_secs;

        if total_nanos >= 1_000_000_000 {
            total_secs += total_nanos / 1_000_000_000;
            total_nanos %= 1_000_000_000;
        }

        TimerDuration::new(total_secs, total_nanos, task_index, timer_index, flags, linked)
    }
}

// Natural ordering: earlier times are less than later times
impl PartialOrd for TimerDuration {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for TimerDuration {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        // Compare seconds first, then nanoseconds
        match self.secs.cmp(&other.secs) {
            std::cmp::Ordering::Equal => self.nanos.cmp(&other.nanos),
            ord => ord,
        }
    }
}

impl Eq for TimerDuration {}

impl PartialEq for TimerDuration {
    fn eq(&self, other: &Self) -> bool {
        self.secs == other.secs && self.nanos == other.nanos
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TimerState {
    Idle,
    Waiting,
    Canceling,
}

pub struct Timer {
    heap: BinaryHeap<Reverse<TimerDuration>>,  // Min-heap using Reverse
    active_timer: Option<TimerDuration>,
    active_timespec: Timespec,
    index: Index,
    state: TimerState,
}

impl Timer {
    pub fn new() -> Self {
        let index = SLAB.with(|slab| {
            let waker = std::task::Waker::from(Arc::new(Waker::new(0)));
            slab.borrow_mut().add(waker)
        });
        Self {
            index,
            heap: BinaryHeap::new(),
            active_timer: None,
            active_timespec: Timespec::new(),
            state: TimerState::Idle
        }
    }

    pub fn is_timer(&self, index: u64) -> bool {
        let index = unsafe { std::mem::transmute::<u64, Index>(index) };
        self.index.index == index.index
    }

    fn cancel_current_timer(&mut self) {
        let Some(timer) = self.active_timer else {
            return;
        };

        println!("Canceling timer: {:?}", timer);
        self.state = TimerState::Canceling;

        POLL.with(|poll| {
            unsafe {
                let entry = io_uring::opcode::TimeoutRemove::new(std::mem::transmute(timer.timer_index))
                    .build()
                    .user_data(std::mem::transmute(timer.timer_index));
                poll.borrow_mut().submission().push(&entry).unwrap();
            }
        });
    }

    fn start_next_timer(&mut self) {
        // Pop the earliest timer from the heap
        let Some(Reverse(timer_duration)) = self.heap.pop() else {
            println!("No more timers in heap, going idle");
            self.state = TimerState::Idle;
            self.active_timer = None;
            self.active_timespec = Timespec::new();
            return;
        };

        println!("Starting next timer: {:?}", timer_duration);
        self.active_timer = Some(timer_duration);
        self.submit_timer(timer_duration);
    }

    fn submit_timer(&mut self, timer: TimerDuration) {
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            unsafe {
                let timespec = timer.to_timespec();
                self.active_timespec = timespec;
                let mut entry = if timer.linked {
                    io_uring::opcode::LinkTimeout::new(&self.active_timespec as _)
                        .build()
                        .user_data(std::mem::transmute(timer.timer_index))
                }else{
                    ITimeout::new(&self.active_timespec as _)
                        .flags(TimeoutFlags::ABS)
                        .build()
                        .user_data(std::mem::transmute(timer.timer_index))
                };
                if let Some(flags) = timer.flags {
                    entry = entry.flags(flags);
                }
                poll.submission().push(&entry).unwrap();
            }
        });
        self.state = TimerState::Waiting;
    }

    fn handle_cancel_completion(&mut self, res: i32) {
        println!("Cancel completed with result: {}", res);

        match -res {
            0 | libc::ENOENT => {
                return
            }
            libc::ECANCELED => {
                let Some(timer) = self.active_timer else {
                    return;
                };
                self.heap.push(Reverse(timer));
                self.start_next_timer();
            }
            libc::ETIME => {
                self.handle_timeout_completion(res);
            }
            _ => {
                unreachable!()
            }
        }
    }

    fn handle_timeout_completion(&mut self, res: i32) {
        println!("Timeout completed with result: {}", res);

        if res != -libc::ETIME {
            println!("Unexpected timeout result: {}", res);
            return;
        }

        let Some(timer) = self.active_timer else {
            println!("Timeout fired but no active timer!");
            return;
        };

        // Wake the task associated with this timer
        SLAB.with(|slab| {
            let slab = slab.borrow();
            if let Some(waker) = slab.get_waker(timer.task_index) {
                println!("Waking task for index: {:?}", timer.task_index);
                waker.wake_by_ref();
            } else {
                println!("No waker found for index: {:?}", timer.task_index);
            }
        });

        // Start the next timer
        self.start_next_timer();
    }

    pub fn handle_event(&mut self, res: i32, index: u64) {
        println!("Timer event - state: {:?}, result: {}", self.state, res);
        if let Some(timer) = self.active_timer {
            let index = unsafe { std::mem::transmute::<u64, Index>(index) };
            let l = index.generation;
            let r = timer.timer_index.generation;
            println!("Got event with generation {l}. Our active timer has generation {r}");
        }

        match self.state {
            TimerState::Idle => {
                println!("Event received while idle - ignoring");
            },
            TimerState::Canceling => {
                self.handle_cancel_completion(res);
            },
            TimerState::Waiting => {
                self.handle_timeout_completion(res);
            },
        }
    }

    pub fn add_duration(&mut self, duration: std::time::Duration, task_index: Index, flags: Option<Flags>, linked: bool) {
        self.index.generation = self.index.generation.wrapping_add(1);
        let new_timer = TimerDuration::from_duration(duration, task_index, self.index, flags, linked);
        println!("Adding timer: {:?}, current state: {:?}", new_timer, self.state);

        match self.state {
            TimerState::Idle => {
                // No active timer, start this one immediately
                self.active_timer = Some(new_timer);
                self.submit_timer(new_timer);
            }
            TimerState::Waiting => {
                let active = self.active_timer.unwrap();
                self.heap.push(Reverse(new_timer));
                if new_timer < active {
                    println!("New timer {:?} is earlier than active {:?}, canceling current timer", new_timer, active);
                    self.cancel_current_timer();
                }
            }
            TimerState::Canceling => {
                // Currently canceling - add to heap, it will be handled after cancel completes
                println!("Currently canceling, adding to heap");
                self.heap.push(Reverse(new_timer));
            }
        }
    }
}

pub fn sleep(duration: std::time::Duration) -> Timeout {
    Timeout::new(duration)
}

pub fn linked_timeout(duration: std::time::Duration) -> LinkedTimeout {
    LinkedTimeout::new(duration)
}
