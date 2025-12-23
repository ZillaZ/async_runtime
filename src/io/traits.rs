pub trait AsyncRead {
    fn read(&mut self, buffer: &mut [u8]) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync;
}

pub trait AsyncWrite {
    fn write(&mut self, buffer: &[u8]) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync;
    fn poll_shutdown(&mut self, how: u32) -> impl Future<Output = Result<(), std::io::Error>> + Send + Sync;
}

pub trait AsyncBufRead<T: Sized + for<'a> From<&'a [u8]>> : AsyncRead {
    fn fill_buf(&mut self) -> impl Future<Output = Result<&[u8], std::io::Error>> + Send + Sync;
    fn consume(&mut self, amount: usize) -> impl Future<Output = ()> + Send + Sync;

    fn has_data_left(&mut self) -> impl Future<Output = Result<bool, std::io::Error>> + Send + Sync where Self: Send + Sync {
        async {
            let bytes = self.fill_buf().await?;
            Ok(!bytes.is_empty())
        }
    }

    fn read_until(&mut self, byte: u8, buf: &mut Vec<u8>) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync where Self: Send + Sync {
        async move {
            let mut consumed = 0;
            loop {
                let bytes = self.fill_buf().await?;
                let (c, found) = match bytes.iter().enumerate().find(|(_, x)| **x == byte) {
                    Some((idx, _)) => {
                        buf.extend(&bytes[..=idx]);
                        consumed += idx;
                        (idx, true)
                    }
                    None => {
                        buf.extend(bytes);
                        consumed += bytes.len();
                        (bytes.len(), false)
                    }
                };
                self.consume(c).await;
                if found {
                    break;
                }
            }
            Ok(consumed)
        }
    }

    fn skip_until(&mut self, byte: u8) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync where Self: Send + Sync {
        async move {
            let mut consumed = 0;
            loop {
                let bytes = self.fill_buf().await?;
                let (c, found) = match bytes.iter().enumerate().find(|(_, x)| **x == byte) {
                    Some((idx, _)) => {
                        consumed += idx;
                        (idx, true)
                    },
                    None => {
                        consumed += bytes.len();
                        (bytes.len(), false)
                    }
                };
                self.consume(c).await;
                if found {
                    break;
                }
            }
            Ok(consumed)
        }
    }

    fn read_line(&mut self, buf: &mut String) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync where Self: Send + Sync {
        async {
            let mut consumed = 0;
            loop {
                let bytes = self.fill_buf().await?;
                let (c, found) = match bytes.iter().enumerate().find(|(_, x)| **x == b'\n') {
                    Some((idx, _)) => {
                        consumed += idx;
                        let Ok(utf8_string) = str::from_utf8(&bytes[..idx]) else {
                            return Err(std::io::ErrorKind::Other.into());
                        };
                        buf.push_str(utf8_string);
                        (idx, true)
                    }
                    None => {
                        consumed += bytes.len();
                        let Ok(utf8_string) = str::from_utf8(&bytes) else {
                            return Err(std::io::ErrorKind::Other.into());
                        };
                        buf.push_str(utf8_string);
                        (bytes.len(), false)
                    }
                };
                self.consume(c).await;
                if found {
                    break;
                }
            }
            Ok(consumed)
        }
    }

    fn split(self, byte: u8) -> impl Future<Output = Split<Self, T>> where Self: Sized + Send + Sync {
        async move {
            Split { r: self, c: byte, __p: std::marker::PhantomData::default() }
        }
    }

    fn lines(self) -> impl Future<Output = Lines<Self, T>> + Send + Sync where Self: Sized + Send + Sync {
        async move {
            Lines { r: self, __p: std::marker::PhantomData::default() }
        }
    }
}

pub trait AsyncIterator {
    type Item;
    fn next(&mut self) -> impl Future<Output = Option<Self::Item>> + Send + Sync;
}

pub struct Lines<R, T> where R: AsyncBufRead<T>, T: for<'a>From<&'a [u8]> {
    r: R,
    __p: std::marker::PhantomData<T>
}

impl <R: AsyncBufRead<T> + Send + Sync, T: Sized + for<'a>From<&'a [u8]>> AsyncIterator for Lines<R, T> {
    type Item = T;
    fn next(&mut self) -> impl Future<Output = Option<Self::Item>> + Send + Sync {
        async {
            let mut buffer = String::new();
            if self.r.read_line(&mut buffer).await.is_err() {
                return None;
            }
            Some(buffer.as_bytes().into())
        }
    }
}

pub struct Split<R, T> where R: AsyncBufRead<T>, T: for<'a>From<&'a [u8]> {
    r: R,
    c: u8,
    __p: std::marker::PhantomData<T>
}

impl <R: AsyncBufRead<T> + Send + Sync, T: Sized + for<'a>From<&'a [u8]>> AsyncIterator for Split<R, T> {
    type Item = T;
    fn next(&mut self) -> impl Future<Output = Option<Self::Item>> + Send + Sync {
        async {
            let mut buffer = Vec::new();
            if self.r.read_until(self.c, &mut buffer).await.is_err() {
                return None;
            };
            Some(buffer.as_slice().into())
        }
    }
}
