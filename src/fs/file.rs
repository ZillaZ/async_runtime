
pub struct File {
    fd: i32,
}

impl File {
    pub fn open<T: ToString>(path: T, flags: i32) -> Result<Self, std::io::Error> {
        use std::str::FromStr;
        let Ok(cstr) = CString::from_str(path.to_string().as_str()) else {
            return Err(std::io::ErrorKind::Other.into())
        };
        let fd = unsafe { libc::open(cstr.as_c_str().as_ptr(), flags) };
        Ok(Self { fd })
    }

    pub fn read<'a>(&self, buffer: &'a mut Vec<u8>) -> FileRead<'a> {
        FileRead { fd: self.fd, buffer, index: None }
    }

    pub fn write<'a>(&self, buffer: &'a Vec<u8>) -> FileWrite<'a> {
        FileWrite { fd: self.fd, buffer, index: None }
    }
}

pub struct FileRead<'a> {
    fd: i32,
    buffer: &'a mut Vec<u8>,
    index: Option<Index>,
}

impl FileRead<'_> {
    fn setup_poll(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            let fd = self.fd;
            unsafe {
                (*sqe).fd = fd;
                (*sqe).len = self.buffer.len() as u32;
                (*sqe).addr_u.addr = std::mem::transmute(self.buffer.as_mut_ptr());
                (*sqe).user_data = std::mem::transmute(self.index.unwrap());
                (*sqe).opcode = IORING_OP_READ;
                (*sqe).off_u.off = u64::MAX;
            }
            poll.register();
        });
    }
}

impl Future for FileRead<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow_mut().get_result(index)
            }).unwrap();
            return if result < 0 {
                StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result)))
            }else{
                StdPoll::Ready(Ok(result as usize))
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

impl Drop for FileRead<'_> {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}

pub struct FileWrite<'a> {
    fd: i32,
    buffer: &'a Vec<u8>,
    index: Option<Index>
}

impl FileWrite<'_> {
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

impl Future for FileWrite<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow_mut().get_result(index)
            }).unwrap();
            return if result < 0 {
                StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result)))
            }else{
                StdPoll::Ready(Ok(result as usize))
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

impl Drop for FileWrite<'_> {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for File {
     fn read<'a>(&mut self, buffer: &'a mut [u8]) -> FdRead<'a> {
         FdRead { fd: self.fd, buffer, index: None }
     }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for File {
    fn write<'a>(&mut self, buffer: &'a [u8]) -> FdWrite<'a> {
         FdWrite { fd: self.fd, index: None, buffer  }
     }
    fn poll_shutdown(&mut self, how: u32) -> PollShutdown {
        PollShutdown { how, fd: self.fd, index: None }
    }
}
