use crate::io::{AsyncBufRead, AsyncRead};
use crate::net::TcpStream;

#[derive(Clone)]
pub struct BufReader<T> where T: Sized + AsyncRead {
    buffer: Vec<u8>,
    reader: T,
}

impl <T> AsyncRead for BufReader<T> where T: Sized + AsyncRead {
    fn read(&mut self, buffer: &mut [u8]) -> impl Future<Output = Result<usize, std::io::Error>> {
        self.reader.read(buffer)
    }
}

impl <T> AsyncBufRead for BufReader<T> where T: Sized + AsyncRead + Send + Sync {
    fn fill_buf(&mut self) -> impl Future<Output = Result<&[u8], std::io::Error>> {
        async {
            let mut buffer : &mut [u8] = &mut [0; 1024];
            let result = self.read(&mut buffer).await;
            if let Err(e) = result {
                if let Some(err) = e.raw_os_error() {
                    if err == -libc::EAGAIN {
                        return if self.buffer.is_empty() {
                            Err(e)
                        }else{
                            Ok(self.buffer.as_slice())
                        }
                    }
                }
                return Err(e);
            }
            let bytes = result.unwrap();
            self.buffer.extend(&buffer[..bytes]);
            Ok(self.buffer.as_slice())
        }
    }

    fn consume(&mut self, amount: usize) -> impl Future<Output = ()> + Send + Sync {
        async move {
            self.buffer = self.buffer[std::cmp::min(self.buffer.len(), amount)..].into();
        }
    }
}

impl From<TcpStream> for BufReader<TcpStream> {
    fn from(value: TcpStream) -> Self {
        Self { reader: value, buffer: Vec::new() }
    }
}
