use std::ffi::CString;
use crate::prelude::{AsyncRead, AsyncWrite};
use crate::descriptors::{FdRead, FdWrite, PollShutdown};

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
}

#[allow(refining_impl_trait)]
impl AsyncRead for File {
     fn read<'a>(&mut self, buffer: &'a mut [u8]) -> FdRead<'a> {
         FdRead::new(self.fd, buffer)
     }

    fn try_read(&mut self, buffer: &mut [u8]) -> impl Future<Output = Option<Result<usize, std::io::Error>>> + Send + Sync {
        async {
            todo!()
        }
    }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for File {
    fn write<'a>(&mut self, buffer: &'a [u8]) -> FdWrite<'a> {
        FdWrite::new(self.fd, buffer)
    }

    fn try_write(&mut self, buffer: &[u8]) -> impl Future<Output = Option<Result<usize, std::io::Error>>> + Send + Sync {
        async {
            todo!()
        }
    }

    fn poll_shutdown(&mut self, how: u32) -> PollShutdown {
        PollShutdown::new(self.fd, how)
    }
}
