use std::io::{Error, ErrorKind};
use std::slice::SliceIndex;
use std::task::{Poll, Waker};
use std::pin::Pin;

use io_uring::squeue::{Entry, Flags};
use io_uring::opcode::{RecvMsg as IRecvMsg, SendMsg as ISendMsg, Accept as IAccept, Bind as IBind, Connect as IConnect, LinkTimeout, Listen as IListen, OpenAt, Read as IRead, Readv, Recv as IRecv, Send as ISend, Shutdown as IShutdown, Socket as ISocket, Timeout as ITimeout, Write as IWrite, Writev};
#[cfg(target_os = "linux")]
use io_uring::types::DestinationSlot;
use io_uring::types::Fd;

use crate::reactor::Index;
use crate::runtime::{POLL, PROBE, SLAB};

macro_rules! impl_drop {
    ($type:ty) => {
        impl Drop for $type {
            fn drop(&mut self) {
                slab_drop(self.index);
            }
        }
    };
    ($type:ty, $($t:ty),*) => {
        impl_drop!($type);
        impl_drop!($($t),*);
    }
}

#[cfg(target_os = "linux")]
pub struct ReadV {
    fd: Fd,
    iovec: Vec<libc::iovec>,
    rw_flags: i32,
    flags: Flags,
    ioprio: u16,
    buf_group: u16,
    offset: u64,
    index: Option<Index>
}

impl ReadV {
    pub fn new(fd: Fd, buffers: Vec<&mut [u8]>) -> Result<Self, Error> {
        probe_is_supported(Readv::CODE)?;
        let iovec = buffers.into_iter().map(|x| {
            libc::iovec { iov_base: x.as_mut_ptr() as _, iov_len: x.len() as _ }
        }).collect::<Vec<_>>();
        Ok(Self { fd, iovec, rw_flags: 0, flags: Flags::empty(), ioprio: 0, buf_group: 0, offset: 0, index: None })
    }

    pub fn rw_flags(&mut self, rw_flags: i32) {
        self.rw_flags = rw_flags;
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }

    pub fn ioprio(&mut self, ioprio: u16) {
        self.ioprio = ioprio;
    }

    pub fn buf_group(&mut self, buf_group: u16) {
        self.buf_group = buf_group;
    }

    pub fn offset(&mut self, offset: u64) {
        self.offset = offset;
    }
}

impl Future for ReadV {
    type Output = Result<u64, Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result == -1 {
                Poll::Ready(Err(Error::last_os_error()))
            }else{
                Poll::Ready(Ok(result as _))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = Readv::new(self.fd, self.iovec.as_ptr(), self.iovec.len() as _)
            .rw_flags(self.rw_flags)
            .buf_group(self.buf_group)
            .ioprio(self.ioprio)
            .offset(self.offset)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

#[cfg(target_os = "linux")]
pub struct WriteV {
    fd: Fd,
    iovec: Vec<libc::iovec>,
    rw_flags: i32,
    flags: Flags,
    ioprio: u16,
    offset: u64,
    index: Option<Index>
}

impl WriteV {
    pub fn new(fd: Fd, buffers: Vec<&mut [u8]>) -> Result<Self, Error> {
        probe_is_supported(Writev::CODE)?;
        let iovec = buffers.into_iter().map(|x| {
            libc::iovec { iov_base: x.as_mut_ptr() as _, iov_len: x.len() as _ }
        }).collect::<Vec<_>>();
        Ok(Self { fd, iovec, rw_flags: 0, flags: Flags::empty(), ioprio: 0, offset: 0, index: None })
    }

    pub fn rw_flags(&mut self, rw_flags: i32) {
        self.rw_flags = rw_flags;
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }

    pub fn ioprio(&mut self, ioprio: u16) {
        self.ioprio = ioprio;
    }

    pub fn offset(&mut self, offset: u64) {
        self.offset = offset;
    }
}

impl Future for WriteV {
    type Output = Result<u64, Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result == -1 {
                Poll::Ready(Err(Error::last_os_error()))
            }else{
                Poll::Ready(Ok(result as _))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = Writev::new(self.fd, self.iovec.as_ptr(), self.iovec.len() as _)
            .rw_flags(self.rw_flags)
            .ioprio(self.ioprio)
            .offset(self.offset)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

#[cfg(target_os = "linux")]
pub struct Recv<'a> {
    fd: Fd,
    buffer: &'a mut [u8],
    buf_group: u16,
    recv_flags: i32,
    flags: Flags,
    index: Option<Index>
}

impl <'a> Recv<'a> {
    pub fn new(fd: Fd, buffer: &'a mut [u8]) -> Result<Self, Error> {
        probe_is_supported(IRecv::CODE)?;
        Ok(Self { fd, buffer, buf_group: 0, recv_flags: 0, flags: Flags::empty(), index: None })
    }

    pub fn recv_flags(&mut self, recv_flags: i32) {
        self.recv_flags = recv_flags;
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }

    pub fn buf_group(&mut self, buf_group: u16) {
        self.buf_group = buf_group;
    }
}

impl Future for Recv<'_> {
    type Output = Result<usize, Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result < 0 {
                Poll::Ready(Err(Error::from_raw_os_error(result)))
            }else{
                Poll::Ready(Ok(result as _))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = IRecv::new(self.fd, self.buffer.as_mut_ptr(), self.buffer.len() as _)
            .flags(self.recv_flags)
            .buf_group(self.buf_group)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

fn probe_is_supported(code: u8) -> Result<(), Error> {
    PROBE.with(|probe| {
        if !probe.borrow_mut().is_supported(code) {
            Err(ErrorKind::Unsupported.into())
        }else{
            Ok(())
        }
    })
}

#[cfg(target_os = "linux")]
pub struct Send<'a> {
    fd: Fd,
    buffer: &'a [u8],
    send_flags: i32,
    flags: Flags,
    dest_addr: Option<libc::sockaddr>,
    dest_addr_len: u32,
    index: Option<Index>
}

impl <'a> Send<'a> {
    pub fn new(fd: Fd, buffer: &'a [u8]) -> Result<Self, Error> {
        probe_is_supported(ISend::CODE)?;
        Ok(Self { fd, buffer, send_flags: 0, flags: Flags::empty(), dest_addr: None, dest_addr_len: 0, index: None })
    }

    pub fn dest_addr(&mut self, dest_addr: libc::sockaddr, len: u32) {
        self.dest_addr = Some(dest_addr);
        self.dest_addr_len = len;
    }

    pub fn send_flags(&mut self, send_flags: i32) {
        self.send_flags = send_flags;
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }
}

impl Future for Send<'_> {
    type Output = Result<usize, Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result == -1 {
                Poll::Ready(Err(Error::last_os_error()))
            }else{
                Poll::Ready(Ok(result as _))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let mut entry = ISend::new(self.fd, self.buffer.as_ptr(), self.buffer.len() as _)
            .flags(self.send_flags);
        if let Some(dest_addr) = self.dest_addr {
            entry = entry.dest_addr(&dest_addr as _).dest_addr_len(self.dest_addr_len);
        }
        let entry = entry.build().flags(self.flags).user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

fn setup_poll(entry: &Entry) {
    POLL.with(|poll| {
        unsafe {
            poll.borrow_mut().submission().push(entry);
        }
    })
}

#[cfg(target_os = "linux")]
pub struct Accept {
    fd: Fd,
    addr: libc::sockaddr,
    index: Option<Index>,
    accept_flags: i32,
    flags: Flags,
    file_index: Option<DestinationSlot>,
    len: u32
}

impl Accept {
    pub fn new(fd: Fd, len: u32) -> Result<Self, Error> {
        probe_is_supported(IAccept::CODE)?;
        Ok(Self { fd, addr: libc::sockaddr { sa_data: [0; 14], sa_family: 0 }, len, accept_flags: 0, flags: Flags::empty(), file_index: None, index: None })
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }

    pub fn accept_flags(&mut self, accept_flags: i32) {
        self.accept_flags = accept_flags;
    }

    pub fn file_index(&mut self, file_index: DestinationSlot) {
        self.file_index = Some(file_index);
    }
}

impl Future for Accept {
    type Output = Result<(Fd, libc::sockaddr), Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result == -1 {
                Poll::Ready(Err(Error::last_os_error()))
            }else{
                Poll::Ready(Ok((Fd(result), self.addr)))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = IAccept::new(self.fd, &mut self.addr as _, &mut self.len as _)
            .flags(self.accept_flags)
            .file_index(self.file_index)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

#[cfg(target_os = "linux")]
pub struct Bind<'a> {
    fd: Fd,
    addr: &'a libc::sockaddr,
    len: u32,
    flags: Flags,
    index: Option<Index>
}

impl <'a> Bind<'a> {
    pub fn new(fd: Fd, addr: &'a libc::sockaddr, len: u32) -> Result<Self, Error> {
        probe_is_supported(IBind::CODE)?;
        Ok(Self { fd, addr, len, flags: Flags::empty(), index: None })
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }
}

impl Future for Bind<'_> {
    type Output = Result<(), Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result == -1 {
                Poll::Ready(Err(Error::last_os_error()))
            }else{
                Poll::Ready(Ok(()))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = IBind::new(self.fd, self.addr as _, self.len)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

fn slab_result(index: Index) -> i32 {
    SLAB.with(|slab| slab.borrow_mut().get_result(index)).unwrap()
}

fn slab_index(waker: Waker) -> Index {
    SLAB.with(|slab| slab.borrow_mut().add(waker))
}

fn slab_drop(index: Option<Index>) {
    if let Some(index) = index {
        let _ = SLAB.try_with(|slab| slab.borrow_mut().clear_index(index));
    }
}

#[cfg(target_os = "linux")]
pub struct Connect<'a> {
    fd: Fd,
    addr: &'a libc::sockaddr,
    len: u32,
    flags: Flags,
    index: Option<Index>
}

impl <'a> Connect<'a> {
    pub fn new(fd: Fd, addr: &'a libc::sockaddr, len: u32) -> Result<Self, Error> {
        probe_is_supported(IConnect::CODE)?;
        Ok(Self { fd, addr, len, flags: Flags::empty(), index: None })
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }
}

impl Future for Connect<'_> {
    type Output = Result<(), Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result == -1 {
                Poll::Ready(Err(Error::last_os_error()))
            }else{
                Poll::Ready(Ok(()))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = IConnect::new(self.fd, self.addr as _, self.len)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

#[cfg(target_os = "linux")]
pub struct Socket {
    domain: i32,
    socket_type: i32,
    protocol: i32,
    socket_flags: i32,
    file_index: Option<DestinationSlot>,
    flags: Flags,
    index: Option<Index>
}

impl Socket {
    pub fn new(domain: i32, socket_type: i32, protocol: i32) -> Result<Self, Error> {
        probe_is_supported(ISocket::CODE)?;
        Ok(Self { domain, socket_type, protocol, socket_flags: 0, file_index: None, flags: Flags::empty(), index: None })
    }

    pub fn socket_flags(&mut self, socket_flags: i32) {
        self.socket_flags = socket_flags;
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }

    pub fn file_index(&mut self, file_index: DestinationSlot) {
        self.file_index = Some(file_index);
    }
}

impl Future for Socket {
    type Output = Result<Fd, Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result == -1 {
                Poll::Ready(Err(Error::last_os_error()))
            }else{
                Poll::Ready(Ok(Fd(result)))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = ISocket::new(self.domain, self.socket_type, self.protocol)
            .flags(self.socket_flags)
            .file_index(self.file_index)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

#[cfg(target_os = "linux")]
pub struct Listen {
    fd: Fd,
    backlog: i32,
    flags: Flags,
    index: Option<Index>
}

impl Listen {
    pub fn new(fd: Fd, backlog: i32) -> Result<Self, Error> {
        probe_is_supported(IListen::CODE)?;
        Ok(Self { fd, backlog, flags: Flags::empty(), index: None })
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }
}

impl Future for Listen {
    type Output = Result<(), Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result == -1 {
                Poll::Ready(Err(Error::last_os_error()))
            }else{
                Poll::Ready(Ok(()))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = IListen::new(self.fd, self.backlog)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

impl_drop!(ReadV, WriteV, Recv<'_>, Send<'_>, Accept, Bind<'_>, Connect<'_>, Socket, Listen);

use crate::runtime::TIMER;

#[cfg(target_os = "linux")]
pub struct Timeout {
    duration: std::time::Duration,
    flags: Option<Flags>,
    index: Option<Index>
}

impl Timeout {
    pub fn new(duration: std::time::Duration) -> Self {
        Self { duration, flags: None, index: None }
    }

    fn setup_poll(&mut self) {
        TIMER.with(|timer| {
            let mut timer = timer.borrow_mut();
            timer.add_duration(self.duration, self.index.unwrap(), self.flags, false);
        });
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = Some(flags);
    }
}

impl Future for Timeout {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if self.index.is_some() {
            return Poll::Ready(().into());
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        return Poll::Pending
    }
}

#[cfg(target_os = "linux")]
pub struct LinkedTimeout {
    duration: std::time::Duration,
    flags: Option<Flags>,
    index: Option<Index>
}

impl LinkedTimeout {
    pub fn new(duration: std::time::Duration) -> Self {
        Self { duration, flags: None, index: None }
    }

    fn setup_poll(&mut self) {
        TIMER.with(|timer| {
            let mut timer = timer.borrow_mut();
            timer.add_duration(self.duration, self.index.unwrap(), self.flags, true);
        });
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = Some(flags);
    }
}

impl Future for LinkedTimeout {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if self.index.is_some() {
            return Poll::Ready(().into());
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        return Poll::Ready(().into())
    }
}

impl_drop!(Timeout, LinkedTimeout);

#[cfg(target_os = "linux")]
pub struct Read<'a> {
    fd: Fd,
    buffer: &'a mut[u8],
    flags: Flags,
    rw_flags: i32,
    ioprio: u16,
    index: Option<Index>
}

impl <'a> Read<'a> {
    pub fn new(fd: Fd, buffer: &'a mut [u8]) -> Result<Read, Error> {
        probe_is_supported(IRead::CODE)?;
        Ok(Self { fd, buffer, flags: Flags::empty(), rw_flags: 0, ioprio: 0, index: None })
    }

    pub fn rw_flags(&mut self, rw_flags: i32) {
        self.rw_flags = rw_flags;
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }

    pub fn ioprio(&mut self, ioprio: u16) {
        self.ioprio = ioprio;
    }
}

impl Future for Read<'_> {
    type Output = Result<usize, Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result < 0 {
                Poll::Ready(Err(Error::from_raw_os_error(result)))
            }else{
                Poll::Ready(Ok(result as _))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = IRead::new(self.fd, self.buffer.as_mut_ptr(), self.buffer.len() as _)
            .ioprio(self.ioprio)
            .rw_flags(self.rw_flags)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

#[cfg(target_os = "linux")]
pub struct Write<'a> {
    fd: Fd,
    buffer: &'a [u8],
    flags: Flags,
    rw_flags: i32,
    ioprio: u16,
    index: Option<Index>
}

impl <'a> Write<'a> {
    pub fn new(fd: Fd, buffer: &'a [u8]) -> Result<Self, Error> {
        probe_is_supported(IWrite::CODE)?;
        Ok(Self { fd, buffer, flags: Flags::empty(), rw_flags: 0, ioprio: 0, index: None })
    }
    pub fn rw_flags(&mut self, rw_flags: i32) {
        self.rw_flags = rw_flags;
    }
    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }
    pub fn ioprio(&mut self, ioprio: u16) {
        self.ioprio = ioprio;
    }
}

impl Future for Write<'_> {
    type Output = Result<usize, Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result < 0 {
                Poll::Ready(Err(Error::from_raw_os_error(result)))
            }else{
                Poll::Ready(Ok(result as _))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = IWrite::new(self.fd, self.buffer.as_ptr(), self.buffer.len() as _)
            .rw_flags(self.rw_flags)
            .ioprio(self.ioprio)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

impl_drop!(Read<'_>, Write<'_>);

pub struct Open {
    path: std::ffi::CString,
    open_flags: i32,
    flags: Flags,
    mode: u32,
    index: Option<Index>
}

impl Open {
    pub fn new<T: ToString>(path: T, open_flags: i32) -> Result<Self, std::io::Error> {
        probe_is_supported(OpenAt::CODE)?;
        use std::str::FromStr;
        let cstr = std::ffi::CString::from_str(&path.to_string())?;
        Ok(Self { path: cstr, open_flags, flags: Flags::empty(), mode: libc::O_RDWR as _, index: None })
    }

    pub fn mode(&mut self, mode: u32) {
        self.mode = mode;
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }
}

impl Future for Open {
    type Output = Result<Fd, std::io::Error>;
    fn poll(mut self: std::pin::Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result != 0 {
                Poll::Ready(Err(std::io::Error::from_raw_os_error(result)))
            }else{
                Poll::Ready(Ok(Fd(result)))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = OpenAt::new(Fd(-1), self.path.as_ptr())
            .flags(self.open_flags)
            .mode(self.mode)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

impl_drop!(Open);

#[cfg(target_os = "linux")]
pub struct Shutdown {
    fd: Fd,
    how: i32,
    flags: Flags,
    index: Option<Index>,
}

impl Shutdown {
    pub fn new(fd: Fd, how: i32) -> Result<Self, Error> {
        probe_is_supported(IShutdown::CODE)?;
        Ok(Self { fd, how, flags: Flags::empty(), index: None })
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }
}

impl Future for Shutdown {
    type Output = Result<(), std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result < 0 {
                Poll::Ready(Err(std::io::Error::from_raw_os_error(result)))
            }else{
                Poll::Ready(Ok(()))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let entry = IShutdown::new(self.fd, self.how)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

impl_drop!(Shutdown);

struct MsgHdr(libc::msghdr);
unsafe impl std::marker::Send for MsgHdr{}
unsafe impl Sync for MsgHdr{}

struct IoVec(libc::iovec);
unsafe impl std::marker::Send for IoVec{}
unsafe impl Sync for IoVec{}

#[cfg(target_os = "linux")]
pub struct RecvMsg<'a> {
    fd: Fd,
    iov: Vec<&'a mut [u8]>,
    msghdr: Option<MsgHdr>,
    libc_iov: Vec<IoVec>,
    recv_flags: u32,
    flags: Flags,
    ioprio: u16,
    buf_group: u16,
    index: Option<Index>
}

impl <'a> RecvMsg<'a> {
    pub fn new(fd: Fd, buffers: Vec<&'a mut [u8]>) -> Result<Self, Error> {
        probe_is_supported(IRecvMsg::CODE)?;
        Ok(Self { fd, iov: buffers, msghdr: None, libc_iov: Vec::new(), recv_flags: 0, flags: Flags::empty(), ioprio: 0, buf_group: 16, index: None })
    }

    pub fn recv_flags(&mut self, recv_flags: u32) {
        self.recv_flags = recv_flags;
    }

    pub fn flags(&mut self, flags: Flags) {
        self.flags = flags;
    }

    pub fn ioprio(&mut self, ioprio: u16) {
        self.ioprio = ioprio;
    }

    pub fn buf_group(&mut self, buf_group: u16) {
        self.buf_group = buf_group;
    }
}

impl Future for RecvMsg<'_> {
    type Output = Result<usize, Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut std::task::Context<'_>) -> Poll<Self::Output> {
        if let Some(index) = self.index {
            let result = slab_result(index);
            return if result < 0 {
                Poll::Ready(Err(Error::from_raw_os_error(result)))
            }else{
                Poll::Ready(Ok(result as _))
            }
        }
        let waker = cx.waker().clone();
        let index = slab_index(waker);
        let mut iov = self.iov.iter_mut().map(|x| {
            libc::iovec { iov_base: x.as_mut_ptr() as _, iov_len: x.len() as _ }
        }).collect::<Vec<_>>();
        let mut msghdr = libc::msghdr { msg_name: std::ptr::null_mut(), msg_namelen: 0, msg_iov: iov.as_mut_ptr(), msg_iovlen: iov.len() as _, msg_control: std::ptr::null_mut(), msg_controllen: 0, msg_flags: 0 };
        self.libc_iov = iov.into_iter().map(|x| IoVec(x)).collect::<Vec<_>>();
        self.msghdr = Some(MsgHdr(msghdr));
        let entry = IRecvMsg::new(self.fd, &mut msghdr as _)
            .flags(self.recv_flags as _)
            .buf_group(self.buf_group)
            .ioprio(self.ioprio)
            .build()
            .flags(self.flags)
            .user_data(index.num());
        self.index = Some(index);
        setup_poll(&entry);
        Poll::Pending
    }
}

impl_drop!(RecvMsg<'_>);
