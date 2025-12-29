pub trait AsyncRead {
    fn read(&mut self, buffer: &mut [u8]) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync;
    fn try_read(&mut self, buffer: &mut [u8]) -> impl Future<Output = Option<Result<usize, std::io::Error>>> + Send + Sync;
}

pub trait AsyncWrite {
    fn write(&mut self, buffer: &[u8]) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync;
    fn try_write(&mut self, buffer: &[u8]) -> impl Future<Output = Option<Result<usize, std::io::Error>>> + Send + Sync;
    fn poll_shutdown(&mut self, how: u32) -> impl Future<Output = Result<(), std::io::Error>> + Send + Sync;
}

pub trait AsyncBufRead : AsyncRead {
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
                let left = self.has_data_left().await.unwrap();
                if !left {
                    break;
                }
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
                let left = self.has_data_left().await.unwrap();
                if !left {
                    break;
                }
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
                println!("Bytes is {}", String::from_utf8(bytes.to_vec()).unwrap());
                let (c, found) = match bytes.iter().enumerate().find(|(_, x)| **x == b'\n') {
                    Some((idx, _)) => {
                        consumed += idx;
                        let Ok(utf8_string) = str::from_utf8(&bytes[..idx]) else {
                            return Err(std::io::ErrorKind::Other.into());
                        };
                        buf.push_str(utf8_string);
                        (idx + 1, true)
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
                let left = self.has_data_left().await.unwrap();
                if !left {
                    break;
                }
                if found {
                    break;
                }
            }
            Ok(consumed)
        }
    }

    fn split(self, byte: u8) -> impl Future<Output = Split<Self>> where Self: Sized + Send + Sync {
        async move {
            Split { r: self, c: byte }
        }
    }

    fn lines(self) -> impl Future<Output = Lines<Self>> + Send + Sync where Self: Sized + Send + Sync {
        async move {
            Lines { r: self }
        }
    }
}

pub trait AsyncIterator {
    type Item;
    fn next(&mut self) -> impl Future<Output = Option<Self::Item>>;
    fn map<T, F>(self, mapper: F) -> Map<Self, Self::Item, F, T> where T: Sized, F: AsyncFn(Self::Item) -> T, Self: Sized;
    fn collect(self) -> impl Future<Output = Vec<Self::Item>>;
}

pub struct Map<Prev, Element, Mapper, Ret> where Mapper: AsyncFn(Element) -> Ret, Ret: Sized, Prev: Sized + AsyncIterator, Element: Sized {
    prev: Prev,
    mapper: Mapper,
    __p: std::marker::PhantomData<(Element, Ret)>
}

impl <Prev, Element, Mapper, Ret> AsyncIterator for Map<Prev, Element, Mapper, Ret>
where Mapper: (AsyncFn(Element) -> Ret) + Send + Sync,
      Ret: Sized + Send + Sync,
      Prev: Sized + AsyncIterator<Item = Element> + Send + Sync,
      Element: Sized + Send + Sync
{
    type Item = Ret;
    fn map<T, F>(self, mapper: F) -> Map<Self, Self::Item, F, T>
    where T: Sized,
          F: AsyncFn(Self::Item) -> T
    {
        Map { prev: self, mapper, __p: std::marker::PhantomData::default() }
    }

    fn next(&mut self) -> impl Future<Output = Option<Self::Item>> {
       async {
           let m = &self.mapper;
           let Some(n) = self.prev.next().await else {
               return None;
           };
           let result = m(n).await;
           Some(result)
       }
    }

    fn collect(mut self) -> impl Future<Output = Vec<Self::Item>> {
        async move {
            let mut ret = Vec::new();
            let m = &self.mapper;
            while let Some(n) = self.prev.next().await {
                ret.push(m(n).await);
            }
            ret
        }
    }
}

pub struct Lines<R> where R: AsyncBufRead {
    r: R,
}

impl <R> Lines<R> where R: AsyncBufRead {
    pub fn reader(self) -> R {
        self.r
    }
}

impl <R: AsyncBufRead + Send + Sync> AsyncIterator for Lines<R> {
    type Item = String;
    fn next(&mut self) -> impl Future<Output = Option<Self::Item>> {
        async {
            let mut buffer = String::new();
            println!("lines next. initialized buffer");
            if self.r.read_line(&mut buffer).await.is_err() {
                return None;
            }
            println!("finished reading line. got {buffer}");
            Some(buffer)
        }
    }

    fn map<T, F>(self, mapper: F) -> Map<Self, Self::Item, F, T>
    where T: Sized,
          F: AsyncFn(Self::Item) -> T
    {
        Map { prev: self, mapper, __p: std::marker::PhantomData::default() }
    }

    fn collect(mut self) -> impl Future<Output = Vec<Self::Item>> {
        async move {
            let mut ret = Vec::new();
            while let Some(n) = self.next().await {
                ret.push(n);
            }
            ret
        }
    }
}

pub struct Split<R> where R: AsyncBufRead {
    r: R,
    c: u8,
}

impl <R: AsyncBufRead + Send + Sync> AsyncIterator for Split<R> {
    type Item = Vec<u8>;
    fn next(&mut self) -> impl Future<Output = Option<Self::Item>> {
        async {
            let mut buffer = Vec::new();
            if self.r.read_until(self.c, &mut buffer).await.is_err() {
                return None;
            };
            Some(buffer)
        }
    }

    fn map<T, F>(self, mapper: F) -> Map<Self, Self::Item, F, T> where T: Sized, F: AsyncFn(Self::Item) -> T {
        Map { prev: self, mapper, __p: std::marker::PhantomData::default() }
    }

    fn collect(mut self) -> impl Future<Output = Vec<Self::Item>> {
        async move {
            let mut ret = Vec::new();
            while let Some(n) = self.next().await {
                ret.push(n);
            }
            ret
        }
    }
}
