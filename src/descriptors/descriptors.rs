use uring_lib::{IORING_OP_READ, IORING_OP_WRITE};
use crate::reactor::Index;
use crate::runtime::{SLAB, POLL};
use std::{task::{Context, Poll as StdPoll}, pin::Pin};

pub struct PollShutdown {
    fd: i32,
    index: Option<Index>,
    how: u32
}

impl PollShutdown {
    pub fn new(fd: i32, how: u32) -> Self {
        Self { fd, how, index: None }
    }
    fn setup_poll(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).opcode = uring_lib::IORING_OP_SHUTDOWN;
                (*sqe).fd = self.fd;
                (*sqe).len = self.how;
            }
            poll.register();
        });
    }
}

impl Future for PollShutdown {
    type Output = Result<(), std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow_mut().get_result(index)
            }).unwrap();
            return if result < 0 {
                StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result)))
            }else{
                StdPoll::Ready(Ok(()))
            }
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        StdPoll::Pending
    }
}

pub struct FdWrite<'a> {
    fd: i32,
    index: Option<Index>,
    buffer: &'a [u8]
}

impl <'a> FdWrite<'a> {
    pub fn new(fd: i32, buffer: &'a [u8]) -> Self {
        Self { fd, buffer, index: None }
    }
    fn setup_poll(&mut self) {
        let fd = self.fd;
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let to_write = self.buffer.len();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).fd = fd;
                (*sqe).len = to_write as u32;
                (*sqe).addr_u.addr = std::mem::transmute(self.buffer.as_ptr());
                (*sqe).opcode = IORING_OP_WRITE;
                (*sqe).user_data = std::mem::transmute(self.index.unwrap());
            }
            poll.register();
        });
    }
}

impl Future for FdWrite<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow_mut().get_result(index)
            }).unwrap();
            if result < 0 {
                return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result as i32)));
            }
            return StdPoll::Ready(Ok(result as usize));
        }
        if self.buffer.is_empty() { return StdPoll::Ready(Ok(0)); }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        return StdPoll::Pending;
    }
}

impl Drop for FdWrite<'_> {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}

pub struct FdRead<'a> {
    fd: i32,
    buffer: &'a mut [u8],
    index: Option<Index>
}

impl <'a> FdRead<'a> {
    pub fn new(fd: i32, buffer: &'a mut [u8]) -> Self {
        Self { fd, buffer, index: None }
    }
    fn setup_poll(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            let fd = self.fd;
            let len = self.buffer.len();
            unsafe {
                (*sqe).fd = fd;
                (*sqe).len = len as u32;
                (*sqe).addr_u.addr = std::mem::transmute(self.buffer.as_mut_ptr());
                (*sqe).user_data = std::mem::transmute(self.index.unwrap());
                (*sqe).opcode = IORING_OP_READ;
                (*sqe).off_u.off = u64::MAX;
            }
            poll.register();
        });
    }
}

impl Future for FdRead<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow_mut().get_result(index)
            }).unwrap();
            if result < 0 {
                return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result as i32)))
            }
            return StdPoll::Ready(Ok(result as usize));
        }
        let waker = cx.waker().clone();
        self.index = SLAB.with(|slab| {
            Some(slab.borrow_mut().add(waker))
        });
        self.setup_poll();
        return StdPoll::Pending;
    }
}

impl Drop for FdRead<'_> {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}
