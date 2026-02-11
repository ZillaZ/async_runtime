use crate::{io_uring::api::{Recv, RecvMsg}, prelude::{AsyncRead, AsyncWrite}};
use io_uring::types::Fd;
use crate::io_uring::api::{Accept, Bind, Connect, Listen, Read, Socket, Write, Shutdown, Send};

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
    fd: Fd,
    addr: libc::sockaddr_in,
    started: bool
}

impl TcpListener {
    pub async fn new(addr: &str) -> Result<Self, std::io::Error> {
        let fd = Socket::new(libc::AF_INET, libc::SOCK_STREAM, 0)?.await?;
        let addr = str_to_sockaddr(addr)?;
        Ok(Self { fd, addr, started: false })
    }

    pub fn reuseaddr(&mut self, val: bool) -> Result<(), std::io::Error> {
        let val = if val { 1 } else { 0 };
        let result = unsafe { libc::setsockopt(self.fd.0, libc::SOL_SOCKET, libc::SO_REUSEADDR, std::mem::transmute(&val), std::mem::size_of::<i32>() as u32) };
        if result < 0 {
            return Err(std::io::Error::last_os_error());
        }
        Ok(())
    }

    pub async fn incoming(&mut self) -> Result<TcpStream, std::io::Error> {
        if !self.started {
            self.started = true;
            unsafe {
                Bind::new(self.fd, std::mem::transmute(&self.addr), std::mem::size_of::<libc::sockaddr_in>() as _)?.await?;
            }
            Listen::new(self.fd, 0)?.await?;
        }
        println!("accepting...");
        let (fd, sockaddr) = Accept::new(self.fd, std::mem::size_of::<libc::sockaddr>() as _)?.await?;
        println!("accepted!");
        Ok(TcpStream::new(fd, Some(sockaddr)))
    }
}

#[derive(Clone)]
pub struct TcpStream {
    fd: Fd,
    _addr: Option<libc::sockaddr>
}

impl TcpStream {
    pub fn new(fd: Fd, _addr: Option<libc::sockaddr>) -> Self {
        Self { fd, _addr }
    }

    pub fn fd(&self) -> i32 {
        self.fd.0
    }

    pub async fn new_client(addr: &str) -> Result<Self, std::io::Error> {
        let fd = Socket::new(libc::AF_INET, libc::SOCK_STREAM, 0)?.await?;
        let sockaddr = str_to_sockaddr(addr)?;
        unsafe {
            Connect::new(fd, std::mem::transmute(&sockaddr), std::mem::size_of::<libc::sockaddr_in>() as _)?.await?;
        }
        Ok(Self::new(fd, None))
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for TcpStream {
    fn read<'a>(&mut self, buffer: &'a mut[u8]) -> RecvMsg<'a> {
        let mut ret = RecvMsg::new(self.fd, vec![buffer]).unwrap();
        ret.recv_flags(libc::MSG_DONTWAIT as _);
        ret
    }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for TcpStream {
    fn write<'a>(&mut self, buffer: &'a [u8]) -> Write<'a> {
        Write::new(self.fd, buffer).unwrap()
    }

    fn poll_shutdown(&mut self, how: u32) -> Shutdown {
        Shutdown::new(self.fd, how as _).unwrap()
    }
}
