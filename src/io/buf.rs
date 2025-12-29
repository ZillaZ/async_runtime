use crate::io::{AsyncBufRead, AsyncRead};
use crate::net::TcpStream;

pub struct BufReader<T> where T: Sized + AsyncRead {
    buffer: Vec<u8>,
    offset: usize,
    reader: T,
}

impl <T> AsyncRead for BufReader<T> where T: Sized + AsyncRead {
    fn read(&mut self, buffer: &mut [u8]) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync {
        self.reader.read(buffer)
    }

    fn try_read(&mut self, buffer: &mut [u8]) -> impl Future<Output = Option<Result<usize, std::io::Error>>> + Send + Sync {
        self.reader.try_read(buffer)
    }
}

impl <T> AsyncBufRead for BufReader<T> where T: Sized + AsyncRead + Send + Sync {
    fn fill_buf(&mut self) -> impl Future<Output = Result<&[u8], std::io::Error>> + Send + Sync {
        async {
            let mut buffer : &mut [u8] = &mut [0; 1024];
            let Some(bytes) = self.try_read(&mut buffer).await else {
                return Ok(&self.buffer[self.offset..]);
            };
            let bytes = bytes?;
            self.buffer.extend(&buffer[..bytes]);
            Ok(&self.buffer[self.offset..])
        }
    }

    fn consume(&mut self, amount: usize) -> impl Future<Output = ()> + Send + Sync {
        async move {
            self.offset += amount;
        }
    }
}

impl From<TcpStream> for BufReader<TcpStream> {
    fn from(value: TcpStream) -> Self {
        Self { reader: value, buffer: Vec::new(), offset: 0 }
    }
}
