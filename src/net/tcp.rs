use crate::prelude::{AsyncRead, AsyncWrite};
use crate::reactor::Index;
use crate::runtime::{POLL, SLAB};
use crate::descriptors::PollShutdown;
use uring_lib::{IORING_OP_LISTEN, IORING_OP_ACCEPT, IORING_OP_CONNECT};
use std::{task::{Context, Poll as StdPoll}, pin::Pin};

pub struct Listener {
    fd: i32,
    index: Option<Index>,
    sockaddr: libc::sockaddr,
    len: u64,
}

impl Listener {
    fn setup_poll(&mut self) {
        let fd = self.fd;
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).opcode = IORING_OP_LISTEN;
                (*sqe).len = 0;
                (*sqe).user_data = std::mem::transmute(self.index.unwrap());
                (*sqe).fd = fd;
            }
            poll.register();
        });
    }

    fn accept(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            let fd = self.fd;
            unsafe {
                let len = std::mem::transmute(&mut self.len);
                (*sqe).opcode = IORING_OP_ACCEPT;
                (*sqe).len = 0;
                (*sqe).user_data = std::mem::transmute(self.index.unwrap());
                (*sqe).off_u.addr2 = len;
                (*sqe).addr_u.addr = std::mem::transmute(&mut self.sockaddr);
                (*sqe).fd = fd;
                (*sqe).oflags.accept_flags = libc::SOCK_NONBLOCK as u32;
            }
            poll.register();
        });
    }
}

impl Future for Listener {
    type Output = Result<TcpStream, ()>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow_mut().get_result(index)
            }).unwrap();
            if result < 0 {
                return StdPoll::Ready(Err(()));
            }
            if result != 0 {
                return StdPoll::Ready(Ok(TcpStream::new(result as i32)))
            }
            self.accept();
            return StdPoll::Pending;
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        StdPoll::Pending
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}

fn str_to_sockaddr(addr: &str) -> Result<libc::sockaddr_in, std::io::Error> {
    let slice = addr.split(":").collect::<Vec<&str>>();
    if slice.len() != 2 {
        return Err(std::io::ErrorKind::InvalidInput.into())
    }
    let Ok(port) = slice[1].parse::<u16>() else {
        return Err(std::io::ErrorKind::InvalidInput.into());
    };
    let port = port.to_be();
    let Ok(addr) = slice[0].parse::<std::net::Ipv4Addr>() else {
        return Err(std::io::ErrorKind::InvalidInput.into());
    };
    let addr = u32::from(addr).to_be();
    let sockaddr = libc::sockaddr_in { sin_port: port, sin_addr: libc::in_addr { s_addr: addr }, sin_zero: [0; 8], sin_family: libc::AF_INET as u16 };
    Ok(sockaddr)
}

pub struct TcpListener {
    fd: i32,
    sockaddr: libc::sockaddr_in
}

impl TcpListener {
    pub fn new(addr: &str) -> Result<Self, std::io::Error> {
        let fd = unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            fd
        };
        let sockaddr = str_to_sockaddr(addr)?;
        Ok(Self { fd, sockaddr })
    }

    pub fn reuseaddr(&mut self, val: bool) -> Result<(), std::io::Error> {
        let val = if val { 1 } else { 0 };
        let result = unsafe { libc::setsockopt(self.fd, libc::SOL_SOCKET, libc::SO_REUSEADDR, std::mem::transmute(&val), std::mem::size_of::<i32>() as u32) };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub fn bind(&mut self) -> Result<(), std::io::Error> {
        unsafe {
            let result = libc::bind(self.fd, std::mem::transmute(&self.sockaddr), std::mem::size_of::<libc::sockaddr_in>() as u32);
            if result < 0 {
                return Err(std::io::Error::last_os_error());
            }
        }
        Ok(())
    }

    pub fn incoming(&mut self) -> Listener {
        Listener { fd: self.fd, sockaddr: libc::sockaddr { sa_data: [0; 14], sa_family: 0 }, len: std::mem::size_of::<libc::sockaddr>() as u64, index: None }
    }
}

#[derive(Clone)]
pub struct TcpStream {
    fd: i32
}

impl TcpStream {
    pub fn new(fd: i32) -> Self {
        Self { fd }
    }

    pub fn fd(&self) -> i32 {
        self.fd
    }

    pub fn new_client(addr: &str) -> Result<SocketConnect, std::io::Error> {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sockaddr = str_to_sockaddr(addr)?;
        Ok(SocketConnect { stream: Self { fd }, index: None, sockaddr, len: std::mem::size_of::<libc::sockaddr_in>() as u32 })
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for TcpStream {
    fn read<'a>(&mut self, buffer: &'a mut[u8]) -> SocketRead<'a> {
        SocketRead::new(self.fd, buffer, true)
    }

    fn try_read<'a>(&mut self, buffer: &'a mut [u8]) -> impl Future<Output = Option<Result<usize, std::io::Error>>> + Send + Sync {
        async {
            let result = SocketRead::new(self.fd, buffer, false).await;
            let Err(e) = result else {
                return Some(result)
            };
            let Some(e) = e.raw_os_error() else {
                return Some(Err(e));
            };
            if e == -libc::EAGAIN || e == -libc::EWOULDBLOCK {
                return None;
            }
            return Some(Err(std::io::Error::from_raw_os_error(e)));
        }
    }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for TcpStream {
    fn write<'a>(&mut self, buffer: &'a [u8]) -> SocketWrite<'a> {
        SocketWrite::new(self.fd, buffer, true)
    }

    fn try_write<'a>(&mut self, buffer: &'a [u8]) -> impl Future<Output = Option<Result<usize, std::io::Error>>> + Send + Sync {
        async {
            let result = SocketWrite::new(self.fd, buffer, false).await;
            let Err(e) = result else {
                return Some(result);
            };
            let Some(e) = e.raw_os_error() else {
                return Some(Err(e));
            };
            if e == -libc::EAGAIN || e == -libc::EWOULDBLOCK {
                return None;
            }
            return Some(Err(std::io::Error::from_raw_os_error(e)));
        }
    }

    fn poll_shutdown(&mut self, how: u32) -> PollShutdown {
        PollShutdown::new(self.fd, how)
    }
}

pub struct SocketConnect {
    stream: TcpStream,
    index: Option<Index>,
    sockaddr: libc::sockaddr_in,
    len: u32
}

impl SocketConnect {
    fn setup_poll(&mut self) {
        let fd = self.stream.fd;
        POLL.with(|poll| {
           let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).fd = fd;
                (*sqe).user_data = std::mem::transmute(self.index.unwrap());
                (*sqe).opcode = IORING_OP_CONNECT;
                (*sqe).len = 0;
                (*sqe).addr_u.addr = std::mem::transmute(&mut self.sockaddr);
                (*sqe).off_u.off = self.len as u64;
            }
            poll.register();
        });
    }
}

impl Future for SocketConnect {
    type Output = Result<TcpStream, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow_mut().get_result(index)
            }).unwrap();
            if result != 0 {
                return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result as i32)));
            }
            return StdPoll::Ready(Ok(self.stream.clone()));
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        StdPoll::Pending
    }
}

impl Drop for SocketConnect {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}

pub struct SocketWrite<'a> {
    fd: i32,
    index: Option<Index>,
    buffer: &'a [u8],
    blocking: bool
}

impl <'a> SocketWrite<'a> {
    pub fn new(fd: i32, buffer: &'a [u8], blocking: bool) -> Self {
        Self { fd, buffer, blocking, index: None }
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
                (*sqe).opcode = uring_lib::IORING_OP_SEND;
                (*sqe).user_data = std::mem::transmute(self.index.unwrap());
            }
            poll.register();
        });
    }
}

impl Future for SocketWrite<'_> {
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

impl Drop for SocketWrite<'_> {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}

pub struct SocketRead<'a> {
    fd: i32,
    buffer: &'a mut [u8],
    index: Option<Index>,
    blocking: bool
}

impl <'a> SocketRead<'a> {
    pub fn new(fd: i32, buffer: &'a mut [u8], blocking: bool) -> Self {
        Self { fd, buffer, blocking, index: None }
    }

    fn setup_poll(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            let fd = self.fd;
            let buf_ptr = self.buffer.as_mut_ptr();
            let buf_len = self.buffer.len();
            let user_data = self.index.unwrap();
            unsafe {
                (*sqe).fd = fd;
                (*sqe).opcode = uring_lib::IORING_OP_RECV;
                (*sqe).addr_u.addr = buf_ptr as u64;
                (*sqe).len = buf_len as u32;
                if !self.blocking {
                    (*sqe).oflags.msg_flags = libc::MSG_DONTWAIT as u32;
                }
                (*sqe).off_u.off = 0;
                (*sqe).user_data = std::mem::transmute(user_data);
            }
            println!("sqe registered");
            poll.register();
        });
    }
}

impl Future for SocketRead<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow_mut().get_result(index)
            }).unwrap();
            println!("result is {result}");
            if result < 0 {
                if result == -libc::EWOULDBLOCK || result == -libc::EAGAIN {
                    return StdPoll::Ready(Ok(0));
                }
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

impl Drop for SocketRead<'_> {
    fn drop(&mut self) {
        let Some(index) = self.index else {
            return;
        };
        let _ = SLAB.try_with(|slab| {
            slab.borrow_mut().clear_index(index);
        });
    }
}
