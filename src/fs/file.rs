use io_uring::types::Fd;

use crate::prelude::{AsyncRead, AsyncWrite};
use crate::io_uring::api::{Open, Read, Shutdown, Write};

pub struct File {
    fd: Fd,
}

impl File {
    pub fn new(fd: Fd) -> Self {
        Self { fd }
    }
    pub async fn open<T: ToString>(path: T, flags: i32) -> Result<Self, std::io::Error> {
        let fd = Open::new(path, flags)?.await?;
        Ok(Self::new(fd))
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for File {
     fn read<'a>(&mut self, buffer: &'a mut [u8]) -> Read<'a> {
         Read::new(self.fd, buffer).unwrap()
     }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for File {
    fn write<'a>(&mut self, buffer: &'a [u8]) -> Write<'a> {
        Write::new(self.fd, buffer).unwrap()
    }

    fn poll_shutdown(&mut self, how: u32) -> Shutdown {
        Shutdown::new(self.fd, how as _).unwrap()
    }
}
