use crate::reactor::Index;
use crate::runtime::{SLAB, POLL};
use std::{task::{Context, Poll as StdPoll}, pin::Pin};
use io_uring::types::Fd;
use io_uring::opcode::{Read, Write, Shutdown};

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
            unsafe {
                let entry = Shutdown::new(Fd(self.fd), self.how as i32)
                    .build()
                    .user_data(std::mem::transmute(self.index.unwrap()));
                poll.submission().push(&entry).unwrap();
            }
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

impl Drop for PollShutdown {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
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
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let to_write = self.buffer.len();
            unsafe {
                let entry = Write::new(Fd(self.fd), self.buffer.as_ptr() as _, to_write as _)
                    .build()
                    .user_data(std::mem::transmute(self.index.unwrap()));
                poll.submission().push(&entry).unwrap();
            }
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
            unsafe {
                let entry = Read::new(Fd(self.fd), self.buffer.as_mut_ptr() as _, self.buffer.len() as _)
                    .build()
                    .user_data(std::mem::transmute(self.index.unwrap()));
                poll.submission().push(&entry).unwrap();
            }
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
