use std::{collections::HashMap, ffi::CString, pin::Pin, sync::{mpsc::{channel, Receiver, Sender}, Arc, LazyLock, Mutex}, task::{Context, Poll as StdPoll, Wake}};
use uring_lib::{read_cq, setup_io_uring, setup_rings, write_sq, IORING_OP_ACCEPT, IORING_OP_CONNECT, IORING_OP_LISTEN, IORING_OP_READ, IORING_OP_WRITE};

pub trait AsyncRead {
    fn read(&mut self) -> impl Future<Output = Result<Vec<u8>, std::io::Error>> + Send + Sync;
}

pub trait AsyncWrite {
    fn write(&mut self, vec: Vec<u8>) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync;
}

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

pub struct FileRead {
    fd: i32,
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

use uring_lib::MutVoidPtr;

#[allow(refining_impl_trait)]
impl AsyncRead for File {
     fn read(&mut self) -> FdRead {
         FdRead { fd: self.fd, inner_buffer: Vec::with_capacity(1024), tb: [0; 1024], len: 1024, info: InfoPtr { processed: false, n: 0, err: 0, buffer: MutVoidPtr(std::ptr::null_mut()) }, started: false, acc: Vec::with_capacity(1024) }
     }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for File {
    fn write(&mut self, vec: Vec<u8>) -> FdWrite {
         FdWrite { fd: self.fd, offset: 0, started: false, info: InfoPtr { processed: false, n: 0, err: 0, buffer: MutVoidPtr(std::ptr::null_mut()) }, inner_buffer: Vec::with_capacity(1024) }
     }
}

pub struct FdRead {
    fd: i32,
    inner_buffer: Vec<u8>,
    tb: [u8; 1024],
    len: usize,
    info: InfoPtr,
    started: bool,
    acc: Vec<u8>,
}

impl FdRead {
    fn setup_poll(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.lock().unwrap();
            let sqe = poll.get_task();
            let fd = self.fd;
            let len = self.len;
            unsafe {
                self.info.buffer = std::mem::transmute(&mut self.tb);
                (*sqe).fd = fd;
                (*sqe).len = len as u32;
                (*sqe).addr_u.addr = std::mem::transmute(self.info.buffer);
                (*sqe).user_data = &mut self.info as *mut InfoPtr as usize as u64;
                (*sqe).opcode = IORING_OP_READ;
                (*sqe).off_u.off = u64::MAX;
            }
            poll.register();
        });
    }
}

use std::ops::{Deref, DerefMut};

impl Future for FdRead {
    type Output = Result<Vec<u8>, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if !self.started {
            self.setup_poll();
            self.started = true;
            return StdPoll::Pending;
        }
        if self.info.err != 0 {
            return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(self.info.err)))
        }
        let b : *mut [u8; 1024] = std::ptr::with_exposed_provenance_mut(self.info.buffer.0 as usize);
        let n = self.info.n;
        unsafe{
            self.inner_buffer.extend(&(&*b)[..n as usize]);
        }
        let i = self.inner_buffer.clone();
        self.acc.extend(i);
        StdPoll::Ready(Ok(self.acc.clone()))
    }
}

pub struct SocketConnect {
    stream: TcpStream,
    started: bool,
    info: InfoPtr,
    sockaddr: libc::sockaddr_in,
    len: u32
}

impl SocketConnect {
    fn setup_poll(&mut self) {
        let fd = self.stream.fd;
        POLL.with(|poll| {
           let mut poll = poll.lock().unwrap();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).fd = fd;
                (*sqe).user_data = &mut self.info as *mut InfoPtr as usize as u64;
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
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        println!("connecting...");
        if !self.started {
            self.setup_poll();
            self.started = true;
            return StdPoll::Pending;
        }
        if !self.info.processed {
            return StdPoll::Pending
        }
        if self.info.err != 0 {
            println!("SocketConnect died with code {}", self.info.err);
            return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(self.info.err)));
        }
        StdPoll::Ready(Ok(self.stream.clone()))
    }
}

pub struct FdWrite {
    fd: i32,
    offset: usize,
    started: bool,
    info: InfoPtr,
    inner_buffer: Vec<u8>
}

impl FdWrite {
    fn setup_poll(&mut self) {
        let fd = self.fd;
        POLL.with(|poll| {
            let mut poll = poll.lock().unwrap();
            let to_write = self.inner_buffer.len() - self.offset;
            unsafe {
                let sqe = poll.get_task();
                (*sqe).fd = fd;
                (*sqe).len = to_write as u32;
                (*sqe).addr_u.addr = std::mem::transmute(self.inner_buffer.as_mut_ptr().wrapping_add(self.offset));
                (*sqe).opcode = IORING_OP_WRITE;
                (*sqe).user_data = &mut self.info as *mut InfoPtr as u64;
            }
            poll.register();
        });
    }
}

impl Future for FdWrite {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        println!("writing...");
        if self.inner_buffer.is_empty() { return StdPoll::Ready(Ok(0)); }
        if !self.started {
            self.setup_poll();
            self.started = true;
            return StdPoll::Pending;
        }
        if !self.info.processed {
            return StdPoll::Pending;
        }
        if self.info.err != 0 {
            return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(self.info.err)));
        }
        self.offset += self.info.n as usize;
        if self.offset == self.inner_buffer.len() {
            return StdPoll::Ready(Ok(self.offset));
        }
        self.info.processed = false;
        self.setup_poll();
        StdPoll::Pending
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

    pub fn new_client(addr: &str) -> Result<SocketConnect, std::io::Error> {
        let fd = unsafe { libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_NONBLOCK, 0) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error());
        }
        let sockaddr = str_to_sockaddr(addr)?;
        Ok(SocketConnect { stream: Self { fd }, started: false, info: InfoPtr { processed: false, n: 0, err: 0, buffer: MutVoidPtr(std::ptr::null_mut()) }, sockaddr, len: std::mem::size_of::<libc::sockaddr_in>() as u32 })
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for TcpStream {
    fn read(&mut self) -> FdRead {
        FdRead { fd: self.fd, acc: Vec::with_capacity(1024), started: false, len: 1024, info: InfoPtr { processed: false, n: 0, err: 0, buffer: MutVoidPtr(std::ptr::null_mut()) }, inner_buffer: Vec::new(), tb: [0; 1024] }
    }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for TcpStream {
    fn write(&mut self, buffer: Vec<u8>) -> FdWrite {
        FdWrite { fd: self.fd, offset: 0, started: false, info: InfoPtr { processed: false, n: 0, err: 0, buffer: MutVoidPtr(std::ptr::null_mut()) }, inner_buffer: buffer }
    }
}

pub struct Listener {
    fd: i32,
    info: InfoPtr,
    started: bool,
    sockaddr: libc::sockaddr,
    len: u64,
}

impl Listener {
    fn setup_poll(&mut self) {
        let fd = self.fd;
        POLL.with(|poll| {
            let mut poll = poll.lock().unwrap();
            let sqe = poll.get_task();
            self.info.n = unsafe { std::mem::transmute(&mut self.info) };
            unsafe {
                (*sqe).opcode = IORING_OP_LISTEN;
                (*sqe).len = 0;
                (*sqe).user_data = &mut self.info as *mut InfoPtr as u64;
                (*sqe).fd = fd;
            }
            poll.register();
        });
    }

    fn accept(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.lock().unwrap();
            let sqe = poll.get_task();
            let fd = self.fd;
            unsafe {
                self.info.buffer = std::mem::transmute(&mut self.sockaddr);
                let len = std::mem::transmute(&mut self.len);
                (*sqe).opcode = IORING_OP_ACCEPT;
                (*sqe).len = 0;
                (*sqe).user_data = &mut self.info as *mut InfoPtr as u64;
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
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if !self.started {
            self.setup_poll();
            self.started = true;
            return StdPoll::Pending
        }
        if !self.info.processed {
            return StdPoll::Pending
        }
        if self.info.err != 0 {
            return StdPoll::Ready(Err(()));
        }
        if self.info.n != 0 {
            return StdPoll::Ready(Ok(TcpStream::new(self.info.n as i32)))
        }
        self.accept();
        self.info.processed = false;
        StdPoll::Pending
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
        Listener { fd: self.fd, sockaddr: libc::sockaddr { sa_data: [0; 14], sa_family: 0 }, len: std::mem::size_of::<libc::sockaddr>() as u64, info: InfoPtr { processed: false, n: 0, err: 0, buffer: MutVoidPtr(std::ptr::null_mut()) }, started: false }
    }
}

pub struct Sleep {
    secs: i64,
    nanos: i64,
    started: bool,
    fd: i32,
    info: InfoPtr
}

impl Sleep {
    fn setup_poll(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.lock().unwrap();
            let timerfd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC) };
            self.fd = timerfd;
            let spec = libc::itimerspec { it_interval: libc::timespec { tv_sec: 0, tv_nsec: 0 }, it_value: { libc::timespec { tv_sec: self.secs, tv_nsec: self.nanos } }  };
            unsafe { libc::timerfd_settime(timerfd, 0, &spec, core::ptr::null_mut()) };
            let sqe = poll.get_task();
            unsafe {
                (*sqe).fd = timerfd;
                (*sqe).opcode = IORING_OP_READ;
                (*sqe).len = std::mem::size_of::<libc::itimerspec>() as u32;
                (*sqe).user_data = &mut self.info as *mut InfoPtr as u64;
                (*sqe).addr_u.addr = std::mem::transmute(&spec);
            }
            poll.register();
        });
    }
}

impl Future for Sleep {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if !self.started {
            self.setup_poll();
            self.started = true;
            return StdPoll::Pending
        }
        if !self.info.processed {
            return StdPoll::Pending
        }
        StdPoll::Ready(().into())
    }
}

pub fn i_sleep(secs: i64, nanos: i64) -> Sleep {
    Sleep { secs, nanos, started: false, fd: 0, info: InfoPtr { processed: false, n: 0, err: 0, buffer: MutVoidPtr(std::ptr::null_mut()) } }
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

type AsyncTask = Pin<Box<dyn Future<Output = ()> + Send + Sync>>;

pub fn sleep(duration: std::time::Duration) -> Sleep {
    let secs_as_nanos = (duration.as_secs() as u128) * (10e8 as u128);
    let nanos = (duration.as_nanos() - secs_as_nanos) as i64;
    i_sleep(duration.as_secs() as i64, nanos)
}

#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct InfoPtr {
    processed: bool,
    n: u64,
    err: i32,
    buffer: uring_lib::MutVoidPtr
}

#[derive(Clone)]
pub struct Poll {
    params: uring_lib::io_uring_params,
    info: uring_lib::uring_queue_info
}

impl Poll {
    pub fn new() -> Self {
        let (fd, mut params) = setup_io_uring(1024).unwrap();
        let info = setup_rings(fd, &mut params).unwrap();
        Self { params, info }
    }

    pub fn poll(&mut self) -> Result<(i32, u64), std::io::Error> {
        read_cq(&self.params, &self.info, 1)
    }

    pub fn register(&mut self) {
        write_sq(&self.params, &mut self.info).unwrap();
    }

    pub fn get_task(&mut self) -> *mut uring_lib::io_uring_sqe {
        uring_lib::get_sqe_tail(&mut self.info)
    }
}

thread_local! {
    pub static TASK_QUEUE : LazyLock<Arc<Mutex<Vec<(u64, AsyncTask)>>>> = LazyLock::new(|| {
        Arc::new(Mutex::new(Vec::new()))
    });

    pub static WAKER : LazyLock<std::task::Waker> = LazyLock::new(|| {
        new_waker()
    });

    pub static POLL : LazyLock<Arc<Mutex<Poll>>> = LazyLock::new(|| {
        Arc::new(Mutex::new(Poll::new()))
    });
    pub static TASKS : std::sync::LazyLock<Arc<Mutex<HashMap<u64, AsyncTask>>>> = LazyLock::new(|| {
        Arc::new(Mutex::new(HashMap::new()))
    });
    pub static TOKEN : LazyLock<Arc<Mutex<u64>>> = LazyLock::new(|| {
        Arc::new(Mutex::new(0))
    });
}

#[macro_export]
macro_rules! initialize_runtime {
    () => {
        let waker = $crate::WAKER.with(|waker| {
            (*waker).clone()
        });
        let mut runtime = NoirRuntime::new(&waker);
        runtime.work();
    }
}

pub struct NoirRuntime<'a> {
    context: Context<'a>,
}

impl <'a> NoirRuntime<'a> {
    pub fn new(waker: &'a std::task::Waker) -> Self {
        Self { context: Context::from_waker(waker) }
    }

    pub fn work(&mut self) {
        let r = self.exec();
        r.iter().for_each(|x| {
            TASKS.with(|tasks| {
                let mut tasks = tasks.lock().unwrap();
                tasks.remove(x);
            });
        });
        loop {
            {
                if TASKS.with(|tasks| {
                    tasks.lock().unwrap().is_empty()
                }) {
                    break
                }
            }
            let Ok((ret, ptr)) = POLL.with(|poll| {
                poll.lock().unwrap().poll()
            }) else {
                continue
            };
            println!("poll result: {ret} {ptr}");

            let ptr : *mut InfoPtr = std::ptr::with_exposed_provenance_mut(ptr as usize);
            unsafe {
                (*ptr).processed = true;
                if ret < 0 {
                    (*ptr).err = ret;
                }else{
                    (*ptr).n = ret as u64;
                }
                println!("{:?}", *ptr);
            }
            let finished = self.exec();
            TASKS.with(|tasks| {
                let mut t = tasks.lock().unwrap();
                for i in finished {
                    t.remove(&i);
                }
            });
        }
    }

    pub fn exec(&mut self) -> Vec<u64> {
        let mut to_remove = vec![];
        TASKS.with(|tasks| {
            let mut tasks = tasks.lock().unwrap();
            for (token, task) in tasks.iter_mut() {
                println!("exec iter");
                match task.as_mut().poll(&mut self.context) {
                    StdPoll::Pending => (),
                    StdPoll::Ready(_) => {
                        to_remove.push(*token);
                    }
                }
            }
            TASK_QUEUE.with(|queue| {
                let mut queue = queue.lock().unwrap();
                while let Some((token, task)) = queue.pop() {
                    tasks.insert(token, task);
                }
            });
        });
        to_remove
    }
}

pub fn new_waker() -> std::task::Waker {
    std::task::Waker::from(Arc::new(Waker::new()))
}

#[macro_export]
macro_rules! push_task {
    ($task:expr) => {
        {
            let mut token = $crate::TOKEN.with(|token| {
                let mut token = token.lock().unwrap();
                *token += 1;
                *token
            });
            let mut tasks = $crate::TASKS.with(|queue| {
                let mut queue = queue.lock().unwrap();
                let pin = Box::pin($task);
                queue.insert(token, pin);
            });
        }
    };
    ($task:expr, $($next:expr),*) => {
        push_task!($runtime, $task);
        push_task!($runtime, $($next),*);
    }
}

#[macro_export]
macro_rules! spawn {
    ($task:expr) => {
        {
            let mut token = $crate::TOKEN.with(|token| {
                let mut token = token.lock().unwrap();
                *token += 1;
                *token
            });
            let mut tasks = $crate::TASK_QUEUE.with(|queue| {
                let mut queue = queue.lock().unwrap();
                println!("SPAWN GOT LOCK");
                let pin = Box::pin($task);
                queue.push((token, pin));
            });
        }
    }
}
