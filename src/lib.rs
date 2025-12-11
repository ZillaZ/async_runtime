use std::{collections::HashMap, ffi::{c_int, CString}, pin::{pin, Pin}, sync::{mpsc::{channel, Receiver, Sender}, Arc}, task::{Context, Poll as StdPoll, Wake}};

pub trait AsyncRead {
    async fn read(&mut self) -> Result<Vec<u8>, std::io::Error>;
}

pub trait AsyncWrite {
    async fn write(&mut self, vec: Vec<u8>) -> Result<usize, std::io::Error>;
}

pub struct File {
    fd: i32,
    token: u64,
    sender: Sender<u64>,
}

impl File {
    pub fn open<T: ToString>(path: T, flags: i32, token: u64, sender: Sender<u64>) -> Result<Self, std::io::Error> {
        use std::str::FromStr;
        let Ok(cstr) = CString::from_str(path.to_string().as_str()) else {
            return Err(std::io::ErrorKind::Other.into())
        };
        let fd = unsafe { libc::open(cstr.as_c_str().as_ptr(), flags) };
        Ok(Self { fd, token, sender })
    }
}

pub struct FileRead {
    fd: i32,
    token: u64,
    sender: Sender<u64>,
    buffer: Vec<u8>
}

impl Future for FileRead {
    type Output = Result<Vec<u8>, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        let mut local = [0; 128];
        let read = unsafe { libc::read(self.fd, std::mem::transmute(&mut local), local.len()) };
        if read < 0 {
            return StdPoll::Ready(Err(std::io::Error::last_os_error()));
        }

        self.buffer.extend(&local[..read as usize]);
        let ret = if read == 0 {
            StdPoll::Ready(Ok(self.buffer.clone()))
        }else{
            StdPoll::Pending
        };
        self.sender.send(self.token).unwrap();
        let waker = cx.waker().clone();
        waker.wake();
        ret
    }
}

pub struct FileWrite {
    fd: i32,
    token: u64,
    sender: Sender<u64>,
    buffer: Vec<u8>
}

impl Future for FileWrite {
    type Output = Result<usize, std::io::Error>;
    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        todo!()
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for File {
     fn read(&mut self) -> FileRead {
         FileRead { fd: self.fd, token: self.token, sender: self.sender.clone(), buffer: Vec::with_capacity(1024) }
     }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for File {
    fn write(&mut self, vec: Vec<u8>) -> FileWrite {
         FileWrite { fd: self.fd, token: self.token, sender: self.sender.clone(), buffer: vec }
     }
}

pub struct FdRead {
    poll: Poll,
    fd: i32,
    buffer: Vec<u8>,
    token: u64,
    started: bool
}

impl Future for FdRead {
    type Output = Result<Vec<u8>, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if !self.started {
            let fd = self.fd;
            let token = self.token;
            self.poll.register(fd, token, libc::EPOLLIN as u32);
            self.started = true;
            return StdPoll::Pending;
        }
        let mut temp : [u8; 1024] = [0; 1024];
        let read = unsafe {
            libc::read(self.fd, std::mem::transmute(temp.as_mut_ptr()), 1024)
        };
        self.buffer.extend(&temp[..read as usize]);
        let read = unsafe {
            libc::read(self.fd, std::mem::transmute(temp.as_mut_ptr()), 1)
        };
        if read > 0 {
            self.buffer.extend(&temp[..read as usize]);
        }
        let err = std::io::Error::last_os_error();
        if (read < 0 && err.kind() == std::io::ErrorKind::WouldBlock) || read == 0 {
            let fd = self.fd;
            self.poll.deregister(fd);
            StdPoll::Ready(Ok(self.buffer.clone()))
        }else{
            let fd = self.fd;
            let token = self.token;
            self.poll.reregister(fd, token, libc::EPOLLIN as u32);
            StdPoll::Pending
        }
    }
}

pub struct SocketConnect {
    stream: TcpStream,
    started: bool
}

impl Future for SocketConnect {
    type Output = Result<TcpStream, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        let fd = self.stream.fd;
        let token = self.stream.token;
        if !self.started {
            self.stream.poll.register(fd, token, libc::EPOLLOUT as u32);
            self.started = true;
            return StdPoll::Pending;
        }
        let (mut val, mut size) = (0, std::mem::size_of::<i32>());
        let result = unsafe { libc::getsockopt(self.stream.fd, libc::SOL_SOCKET, libc::SO_ERROR, std::mem::transmute(&mut val), std::mem::transmute(&mut size)) };
        if result < 0 {
            println!("ERROR {result}");
            let err = std::io::Error::last_os_error();
            if let Some(e) = err.raw_os_error() {
                if e == libc::EINPROGRESS {
                    self.stream.poll.reregister(fd, token, libc::EPOLLOUT as u32);
                    return StdPoll::Pending;
                }
            }
            self.stream.poll.deregister(fd);
            return StdPoll::Ready(Err(err));
        }
        self.stream.poll.deregister(fd);
        StdPoll::Ready(Ok(self.stream.clone()))
    }
}

pub struct FdWrite {
    poll: Poll,
    fd: i32,
    buffer: Vec<u8>,
    token: u64,
    offset: usize,
    started: bool
}

impl Future for FdWrite {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if !self.started {
            let fd = self.fd;
            let token = self.token;
            self.poll.register(fd, token, libc::EPOLLOUT as u32);
            self.started = true;
            return StdPoll::Pending;
        }
        let offset = self.offset;
        let buffer = self.buffer[offset..].as_mut_ptr();
        let written = unsafe { libc::write(self.fd, std::mem::transmute(buffer), self.buffer.len() - offset) };
        if written < 0 {
            let error = std::io::Error::last_os_error();
            return if error.kind() == std::io::ErrorKind::WouldBlock {
                StdPoll::Ready(Ok(self.offset))
            }else{
                let fd = self.fd;
                self.poll.deregister(fd);
                StdPoll::Ready(Err(error))
            }
        }
        self.offset += written as usize;
        if offset == self.buffer.len() {
            let fd = self.fd;
            self.poll.deregister(fd);
            StdPoll::Ready(Ok(self.offset))
        }else{
            let fd = self.fd;
            let token = self.token;
            self.poll.reregister(fd, token, libc::EPOLLOUT as u32);
            StdPoll::Pending
        }
    }
}

#[derive(Clone)]
pub struct TcpStream {
    poll: Poll,
    fd: i32,
    token: u64
}

impl TcpStream {
    pub fn new(poll: Poll, fd: i32, token: u64) -> Self {
        Self { poll, fd, token }
    }

    pub fn new_client(poll: Poll, addr: &str, token: u64) -> Result<Self, std::io::Error> {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sockaddr = str_to_sockaddr(addr)?;
        let conn_result = unsafe { libc::connect(fd, std::mem::transmute(&sockaddr), std::mem::size_of::<libc::sockaddr_in>() as u32) };
        if conn_result < 0 && conn_result != libc::EINPROGRESS {
            let err = std::io::Error::last_os_error();
            if err.raw_os_error().unwrap() != libc::EINPROGRESS {
                return Err(err);
            }
        }
        Ok(Self { poll, fd, token })
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for TcpStream {
    fn read(&mut self) -> FdRead {
        FdRead { poll: self.poll.clone(), fd: self.fd, token: self.token, buffer: Vec::with_capacity(1024), started: false }
    }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for TcpStream {
    fn write(&mut self, buffer: Vec<u8>) -> FdWrite {
        FdWrite { poll: self.poll.clone(), fd: self.fd, token: self.token, buffer, offset: 0, started: false }
    }
}

pub struct Listener {
    poll: Poll,
    fd: i32,
    token: u64,
}

impl Future for Listener {
    type Output = Result<TcpStream, ()>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        let fd = self.fd;
        let token = self.token;
        let mut peer_addr = libc::sockaddr { sa_data: [0; 14], sa_family: 0 };
        let mut len = std::mem::size_of::<libc::sockaddr_in>() as _;
        let socket = unsafe { libc::accept4(fd, &mut peer_addr, &mut len, libc::SOCK_NONBLOCK) };
        self.poll.reregister(fd, token, libc::EPOLLIN as u32);
        if socket < 0 {
            let err = std::io::Error::last_os_error();
            if err.kind() == std::io::ErrorKind::WouldBlock {
                StdPoll::Pending
            }else{
                StdPoll::Ready(Err(()))
            }
        }else{
            StdPoll::Ready(Ok(TcpStream::new(self.poll.clone(), socket, token)))
        }
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
    poll: Poll,
    fd: i32,
    sockaddr: libc::sockaddr_in,
    token: u64
}

impl TcpListener {
    pub fn new(poll: Poll, addr: &str, token: u64) -> Result<Self, std::io::Error> {
        let fd = unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0);
            if fd < 0 {
                return Err(std::io::Error::last_os_error());
            }
            fd
        };
        let sockaddr = str_to_sockaddr(addr)?;
        Ok(Self { poll, fd, sockaddr, token })
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
            self.poll.register(self.fd, self.token, libc::EPOLLIN as u32);
            libc::listen(self.fd, 1024);
        }
        Ok(())
    }

    pub fn incoming(&mut self) -> Listener {
        Listener { poll: self.poll.clone(), fd: self.fd, token: self.token }
    }
}

pub struct Sleep {
    poll: Poll,
    token: u64,
    secs: i64,
    nanos: i64,
    started: bool,
    fd: i32
}

impl Future for Sleep {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if self.started {
            let fd = self.fd;
            self.poll.deregister(fd);
            return StdPoll::Ready(().into());
        }
        self.started = true;
        let timerfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC) };
        self.fd = timerfd;
        let spec = libc::itimerspec { it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 }, it_value: { libc::timespec { tv_sec: self.secs, tv_nsec: self.nanos } }  };
        unsafe { libc::timerfd_settime(timerfd, 0, &spec, core::ptr::null_mut()) };
        let token = self.token;
        self.poll.register(timerfd, token, libc::EPOLLIN as u32);
        StdPoll::Pending
    }
}

pub fn sleep(poll: Poll, secs: i64, nanos: i64, token: u64) -> Sleep {
    Sleep { poll, token, secs, nanos, started: false, fd: 0 }
}

#[derive(Clone)]
pub struct Waker {
    thread: std::thread::Thread
}

impl Waker {
    pub fn new() -> Self {
        Self { thread: std::thread::current() }
    }
}

impl Wake for Waker {
    fn wake(self: std::sync::Arc<Self>) {
        self.thread.unpark();
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        self.thread.unpark();
    }
}

type AsyncTask<'a> = Pin<&'a mut dyn Future<Output = ()>>;

#[derive(Clone)]
pub struct Sleeper {
    poll: Poll,
    token: u64
}

impl Sleeper {
    pub fn new(poll: Poll, token: u64) -> Self {
        Self { poll, token }
    }

    pub fn sleep(&self, duration: std::time::Duration) -> Sleep {
        let secs_as_nanos = (duration.as_secs() as u128) * (10e8 as u128);
        let nanos = (duration.as_nanos() - secs_as_nanos) as i64;
        sleep(self.poll.clone(), duration.as_secs() as i64, nanos, self.token)
    }
}

#[derive(Clone)]
pub struct Utils {
    poll: Poll,
    token: u64,
    sleeper: Sleeper,
    signal_sender: Sender<u64>,
}

impl Utils {
    pub fn new(poll: Poll, token: u64, signal_sender: Sender<u64>) -> Self {
        Utils { token, signal_sender, poll: poll.clone(), sleeper: Sleeper::new(poll, token) }
    }

    pub fn sleep(&self, duration: std::time::Duration) -> Sleep {
        self.sleeper.sleep(duration)
    }

    pub fn new_tcp_listener(&self, addr: &str) -> Result<TcpListener, std::io::Error> {
        TcpListener::new(self.poll.clone(), addr, self.token)
    }

    pub fn open_file<T: ToString>(&self, path: T) -> Result<File, std::io::Error> {
        File::open(path, libc::O_RDWR, self.token, self.signal_sender.clone())
    }

    pub fn new_tcp_client(&self, addr: &str) -> SocketConnect {
        let stream = TcpStream::new_client(self.poll.clone(), addr, self.token).unwrap();
        SocketConnect { stream, started: false }
    }
}

#[derive(Clone)]
pub struct Poll {
    epfd: i32,
    count: i32
}

impl Poll {
    pub fn new() -> Self {
        let epfd = unsafe { libc::epoll_create(1024) };
        Self { epfd, count: 0 }
    }

    pub fn poll(&mut self, events: &mut [libc::epoll_event; 1024]) -> i32 {
        self.count = unsafe { libc::epoll_wait(self.epfd, events.as_mut_ptr(), 1024, -1) };
        self.count
    }

    pub fn register(&mut self, fd: i32, token: u64, interest: u32) {
        let mut event = libc::epoll_event { events: interest, u64: token };
        unsafe { libc::epoll_ctl(self.epfd, libc::EPOLL_CTL_ADD, fd, &mut event); }
    }

    pub fn reregister(&mut self, fd: i32, token: u64, interest: u32) {
        let mut event = libc::epoll_event { events: interest, u64: token };
        unsafe { libc::epoll_ctl(self.epfd, libc::EPOLL_CTL_MOD, fd, &mut event); }
    }

    pub fn deregister(&mut self, fd: i32) {
        let mut event = libc::epoll_event { events: 0, u64: 0 };
        unsafe { libc::epoll_ctl(self.epfd, libc::EPOLL_CTL_DEL, fd, &mut event); }
    }
}

pub struct NoirRuntime<'a> {
    context: Context<'a>,
    tasks: HashMap<u64, AsyncTask<'a>>,
    signal_sender: Sender<u64>,
    signal_receiver: Receiver<u64>,
    poll: Poll,
    token: u64,
    events: [libc::epoll_event; 1024]
}

impl <'a> NoirRuntime<'a> {
    pub fn new(waker: &'a std::task::Waker) -> Self {
        let (signal_sender, signal_receiver) = channel();
        Self { tasks: HashMap::new(), context: Context::from_waker(waker), poll: Poll::new(), token: 0, events: [libc::epoll_event { events: 0, u64: 0 }; 1024], signal_sender, signal_receiver }
    }

    pub fn get_utils(&mut self) -> Utils {
        self.token += 1;
        Utils::new(self.poll.clone(), self.token, self.signal_sender.clone())
    }

    pub fn task(&mut self, task: AsyncTask<'a>) {
        self.tasks.insert(self.token, task);
    }

    pub fn work(&mut self) {
        self.exec();
        while !self.tasks.is_empty() {
            let mut finished = vec![];
            while let Ok(token) = self.signal_receiver.try_recv() {
                if self.exec_one(token) {
                    finished.push(token);
                }
            }
            for i in finished.iter() {
                self.tasks.remove(i);
            }
            let event_count = self.poll.poll(&mut self.events);
            let mut state = 0;
            for event in self.events {
                if state == event_count { break; }
                state += 1;
                let token = event.u64;
                if self.exec_one(token) {
                    finished.push(token);
                }
            }
            for i in finished {
                self.tasks.remove(&i);
            }
        }
    }

    pub fn exec_one(&mut self, token: u64) -> bool {
        let Some(task) = self.tasks.get_mut(&token) else {
            return false;
        };
        match task.as_mut().poll(&mut self.context) {
            StdPoll::Pending => false,
            StdPoll::Ready(_) => true
        }
    }

    pub fn exec(&mut self) {
        let mut to_remove = vec![];
        for (token, task) in self.tasks.iter_mut() {
            match task.as_mut().poll(&mut self.context) {
                StdPoll::Pending => (),
                StdPoll::Ready(_) => {
                    to_remove.push(*token);
                }
            }
        }
        to_remove.into_iter().for_each(|x| { self.tasks.remove(&x); });
    }
}

pub fn new_waker() -> std::task::Waker {
    std::task::Waker::from(Arc::new(Waker::new()))
}

#[macro_export]
macro_rules! push_task {
    ($runtime:ident, $task:expr) => {
        let utils = $runtime.get_utils();
        let pin = pin!($task(utils));
        {
            $runtime.task(pin);
        }
    };
    ($runtime:ident, $task:expr, $($next:expr),*) => {
        push_task!($runtime, $task);
        push_task!($runtime, $($next),*);
    }
}
