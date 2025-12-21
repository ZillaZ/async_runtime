use std::{collections::HashMap, ffi::CString, pin::Pin, sync::{Arc, LazyLock}, task::{Context, Poll as StdPoll, Wake}};
use uring_lib::{read_cq, setup_io_uring, setup_rings, write_sq, IORING_OP_ACCEPT, IORING_OP_CONNECT, IORING_OP_LISTEN, IORING_OP_READ, IORING_OP_WRITE};

pub trait AsyncRead {
    fn read(&mut self, buffer: &mut [u8]) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync;
}

pub trait AsyncWrite {
    fn write(&mut self, buffer: &[u8]) -> impl Future<Output = Result<usize, std::io::Error>> + Send + Sync;
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

    pub fn read<'a>(&self, buffer: &'a mut Vec<u8>) -> FileRead<'a> {
        FileRead { fd: self.fd, buffer, index: None }
    }

    pub fn write<'a>(&self, buffer: &'a Vec<u8>) -> FileWrite<'a> {
        FileWrite { fd: self.fd, buffer, index: None }
    }
}

pub struct FileRead<'a> {
    fd: i32,
    buffer: &'a mut Vec<u8>,
    index: Option<u64>,
}

impl FileRead<'_> {
    fn setup_poll(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            let fd = self.fd;
            unsafe {
                (*sqe).fd = fd;
                (*sqe).len = self.buffer.len() as u32;
                (*sqe).addr_u.addr = std::mem::transmute(self.buffer.as_mut_ptr());
                (*sqe).user_data = self.index.unwrap();
                (*sqe).opcode = IORING_OP_READ;
                (*sqe).off_u.off = u64::MAX;
            }
            poll.register();
        });
    }
}

impl Future for FileRead<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow().get_result(index)
            }).unwrap();
            return if result < 0 {
                StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result)))
            }else{
                StdPoll::Ready(Ok(result as usize))
            }
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        StdPoll::Pending
    }
}

pub struct FileWrite<'a> {
    fd: i32,
    buffer: &'a Vec<u8>,
    index: Option<u64>
}

impl FileWrite<'_> {
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
                (*sqe).opcode = IORING_OP_WRITE;
                (*sqe).user_data = self.index.unwrap();
            }
            poll.register();
        });
    }
}

impl Future for FileWrite<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow().get_result(index)
            }).unwrap();
            return if result < 0 {
                StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result)))
            }else{
                StdPoll::Ready(Ok(result as usize))
            }
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        StdPoll::Pending
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for File {
     fn read<'a>(&mut self, buffer: &'a mut [u8]) -> FdRead<'a> {
         FdRead { fd: self.fd, buffer, index: None }
     }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for File {
    fn write<'a>(&mut self, buffer: &'a [u8]) -> FdWrite<'a> {
         FdWrite { fd: self.fd, index: None, buffer  }
     }
}

pub struct FdRead<'a> {
    fd: i32,
    buffer: &'a mut [u8],
    index: Option<u64>
}

impl FdRead<'_> {
    fn setup_poll(&mut self) {
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            let fd = self.fd;
            let len = self.buffer.len();
            unsafe {
                (*sqe).fd = fd;
                (*sqe).len = len as u32;
                (*sqe).addr_u.addr = std::mem::transmute(self.buffer.as_mut_ptr());
                (*sqe).user_data = self.index.unwrap();
                (*sqe).opcode = IORING_OP_READ;
                (*sqe).off_u.off = u64::MAX;
            }
            poll.register();
        });
    }
}

impl Future for FdRead<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            let result = SLAB.with(|slab| {
                slab.borrow().get_result(index)
            }).unwrap();
            if result < 0 {
                return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result as i32)))
            }
            return StdPoll::Ready(Ok(result as usize));
        }
        let waker = cx.waker().clone();
        self.index = SLAB.with(|slab| {
            Some(slab.borrow_mut().add(waker))
        });
        self.setup_poll();
        println!("we are about to read some shit. we have index {:?}", self.index);
        return StdPoll::Pending;
    }
}

pub struct SocketConnect {
    stream: TcpStream,
    index: Option<u64>,
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
                (*sqe).user_data = self.index.unwrap();
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
                slab.borrow().get_result(index)
            }).unwrap();
            if result != 0 {
                println!("SocketConnect died with code {}", result);
                return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result as i32)));
            }
            println!("connect is finishing just fine");
            return StdPoll::Ready(Ok(self.stream.clone()));
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        println!("connect started with index {:?}", self.index);
        self.setup_poll();
        StdPoll::Pending
    }
}

pub struct FdWrite<'a> {
    fd: i32,
    index: Option<u64>,
    buffer: &'a [u8]
}

impl FdWrite<'_> {
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
                (*sqe).opcode = IORING_OP_WRITE;
                (*sqe).user_data = self.index.unwrap();
            }
            poll.register();
        });
    }
}

impl Future for FdWrite<'_> {
    type Output = Result<usize, std::io::Error>;
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if let Some(index) = self.index {
            println!("fdwrite with index {index}");
            let result = SLAB.with(|slab| {
                slab.borrow().get_result(index)
            }).unwrap();
            if result < 0 {
                return StdPoll::Ready(Err(std::io::Error::from_raw_os_error(result as i32)));
            }
            return StdPoll::Ready(Ok(result as usize));
        }
        println!("writing...");
        if self.buffer.is_empty() { return StdPoll::Ready(Ok(0)); }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        return StdPoll::Pending;
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
        Ok(SocketConnect { stream: Self { fd }, index: None, sockaddr, len: std::mem::size_of::<libc::sockaddr_in>() as u32 })
    }
}

#[allow(refining_impl_trait)]
impl AsyncRead for TcpStream {
    fn read<'a>(&mut self, buffer: &'a mut[u8]) -> FdRead<'a> {
        FdRead { fd: self.fd, index: None, buffer }
    }
}

#[allow(refining_impl_trait)]
impl AsyncWrite for TcpStream {
    fn write<'a>(&mut self, buffer: &'a [u8]) -> FdWrite<'a> {
        FdWrite { fd: self.fd, index: None, buffer }
    }
}

pub struct Listener {
    fd: i32,
    index: Option<u64>,
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
                (*sqe).user_data = self.index.unwrap();
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
                (*sqe).user_data = self.index.unwrap();
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
                slab.borrow().get_result(index)
            }).unwrap();
            println!("received update on listener future. result is {result} and index is {index}");
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
        println!("io uring received some shit. we have index {:?}", self.index);
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
        Listener { fd: self.fd, sockaddr: libc::sockaddr { sa_data: [0; 14], sa_family: 0 }, len: std::mem::size_of::<libc::sockaddr>() as u64, index: None }
    }
}

use std::collections::BinaryHeap;

#[derive(Clone, Copy)]
struct TimerDuration {
    secs: i64,
    nanos: i64,
    index: u64,
    timespec: libc::timespec
}

impl PartialOrd for TimerDuration {
    fn ge(&self, other: &Self) -> bool {
        let (secs, nanos) = ((self.secs - other.secs), (self.nanos - other.nanos));
        secs >= 0 || (secs == 0 && nanos >= 0)
    }
    fn gt(&self, other: &Self) -> bool {
        let (secs, nanos) = ((self.secs - other.secs), (self.nanos - other.nanos));
        secs > 0 || (secs == 0 && nanos > 0)
    }
    fn le(&self, other: &Self) -> bool {
        let (secs, nanos) = ((self.secs - other.secs), (self.nanos - other.nanos));
        secs <= 0 || (secs == 0 && nanos <= 0)
    }
    fn lt(&self, other: &Self) -> bool {
        let (secs, nanos) = ((self.secs - other.secs), (self.nanos - other.nanos));
        secs < 0 || (secs == 0 && nanos < 0)
    }
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        if self.gt(other) {
            Some(std::cmp::Ordering::Greater)
        }else if self.lt(other){
            Some(std::cmp::Ordering::Less)
        }else if self.eq(other) {
            Some(std::cmp::Ordering::Equal)
        }else{
            None
        }
    }
}

impl Eq for TimerDuration {}

impl PartialEq for TimerDuration {
    fn eq(&self, other: &Self) -> bool {
        self.secs == other.secs && self.nanos == other.nanos
    }

    fn ne(&self, other: &Self) -> bool {
        !self.eq(other)
    }
}

impl TimerDuration {
    fn new(secs: i64, nanos: i64, index: u64) -> Self {
        let timespec = libc::timespec { tv_sec: secs, tv_nsec: nanos };
        TimerDuration { secs, nanos, index, timespec }
    }
}

impl Ord for TimerDuration {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        if (self.secs, self.nanos) == (other.secs, other.nanos) {
            return std::cmp::Ordering::Equal;
        }
        let sub = ((self.secs - other.secs), (self.nanos - other.nanos));
        if sub.0 <= 0 && sub.1 < 0 {
            std::cmp::Ordering::Greater
        }else{
            std::cmp::Ordering::Less
        }
    }
}

enum TimerState {
    Idle,
    Waiting,
    Canceling,
}

struct Timer {
    fd: i32,
    heap: BinaryHeap<TimerDuration>,
    last: Option<TimerDuration>,
    index: u64,
    state: TimerState
}

impl Timer {
    fn new() -> Self {
        let fd = unsafe { libc::timerfd_create(libc::CLOCK_MONOTONIC, libc::TFD_NONBLOCK | libc::TFD_CLOEXEC) };
        let index = SLAB.with(|slab| {
            let waker = std::task::Waker::from(Arc::new(Waker::new(0)));
            slab.borrow_mut().add(waker)
        });
        Self { fd, index, heap: BinaryHeap::new(), last: None, state: TimerState::Idle }
    }

    fn is_timer(&self, index: u64) -> bool {
        self.index == index
    }

    fn cancel_timer(&mut self) {
        let Some(last) = self.last else {
            return;
        };
        self.state = TimerState::Canceling;
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).addr_u.addr = self.index;
                (*sqe).off_u.addr2 = std::mem::transmute(&last.timespec);
                (*sqe).opcode = uring_lib::IORING_OP_TIMEOUT_REMOVE;
                (*sqe).user_data = self.index;
            }
            poll.register();
        });
    }

    fn next_timer(&mut self) {
        let Some(timer_duration) = self.heap.pop() else {
            self.state = TimerState::Idle;
            self.last = None;
            return;
        };
        self.last = Some(timer_duration);
        self.setup_timer();
    }

    fn setup_timer(&mut self) {
        let Some(timer_duration) = self.last else {
            return;
        };
        POLL.with(|poll| {
            let mut poll = poll.borrow_mut();
            let sqe = poll.get_task();
            unsafe {
                (*sqe).fd = self.fd;
                (*sqe).opcode = uring_lib::IORING_OP_TIMEOUT;
                (*sqe).len = 1;
                (*sqe).user_data = self.index;
                (*sqe).addr_u.addr = std::mem::transmute(&timer_duration.timespec);
                (*sqe).oflags.timeout_flags = uring_lib::IORING_TIMEOUT_ABS;
            }
            poll.register();
            self.state = TimerState::Waiting;
        });
    }

    fn handle_cancel(&mut self, res: i32) {
        if res != -libc::ECANCELED && res != -libc::ETIME { return }
        self.setup_timer();
    }

    fn wakeup(&mut self, res: i32) {
        let Some(timer_duration) = self.last else {
            return;
        };
        SLAB.with(|slab| {
            let slab = slab.borrow();
            let waker = slab.get_waker(timer_duration.index).unwrap();
            waker.wake_by_ref();
        });
        self.next_timer();
    }

    fn handle_event(&mut self, res: i32) {
        match self.state {
            TimerState::Idle => return,
            TimerState::Canceling => self.handle_cancel(res),
            TimerState::Waiting => self.wakeup(res),
        }
    }

    fn add_duration(&mut self, duration: std::time::Duration, index: u64) {
        println!("ADD DURATION");
        let (secs, nanos) = duration_to_secs_nanos(duration);
        let mut timespec = libc::timespec { tv_sec: secs, tv_nsec: nanos };
        unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, std::mem::transmute(&mut timespec)); }
        let (secs, nanos) = (secs + timespec.tv_sec, nanos + timespec.tv_nsec);
        let timer_duration = TimerDuration::new(secs, nanos, index);
        if let Some(last) = self.last {
            if timer_duration < last {
                println!("DURATION IS SHORTER GOTTA CANCEL");
                self.cancel_timer();
                self.heap.push(last);
                self.last = Some(timer_duration);
            }else{
                self.heap.push(timer_duration);
            }
        }else {
            self.last = Some(timer_duration);
            self.setup_timer();
        }
    }
}

pub struct Sleep {
    duration: std::time::Duration,
    index: Option<u64>
}

impl Sleep {
    fn setup_poll(&mut self) {
        TIMER.with(|timer| {
            let mut timer = timer.borrow_mut();
            timer.add_duration(self.duration, self.index.unwrap());
        });
    }
}

impl Future for Sleep {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> StdPoll<Self::Output> {
        if self.index.is_some() {
            return StdPoll::Ready(().into());
        }
        let waker = cx.waker().clone();
        self.index = Some(SLAB.with(|slab| {
            slab.borrow_mut().add(waker)
        }));
        self.setup_poll();
        return StdPoll::Pending
    }
}

#[derive(Clone)]
pub struct Waker {
    token: u64
}

impl Waker {
    pub fn new(token: u64) -> Self {
        Self { token }
    }
}

impl Wake for Waker {
    fn wake(self: std::sync::Arc<Self>) {
        COMPLETION_QUEUE.with(|queue| {
            queue.borrow_mut().push(self.token);
        });
    }

    fn wake_by_ref(self: &std::sync::Arc<Self>) {
        COMPLETION_QUEUE.with(|queue| {
            println!("we pushing token {} to the completion queue", self.token);
            queue.borrow_mut().push(self.token);
        });
    }
}

type AsyncTask = Pin<Box<dyn Future<Output = ()> + Send + Sync>>;

fn duration_to_secs_nanos(duration: std::time::Duration) -> (i64, i64) {
    let secs_as_nanos = (duration.as_secs() as u128) * (10e8 as u128);
    let nanos = (duration.as_nanos() - secs_as_nanos) as i64;
    (duration.as_secs() as i64, nanos)
}

pub fn sleep(duration: std::time::Duration) -> Sleep {
    Sleep { duration, index: None }
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

pub struct Slab {
    index: u64,
    cache: Vec<u64>,
    wakers: Vec<std::task::Waker>,
    results: HashMap<u64, i32>,
    wakers_by_token: HashMap<u64, std::task::Waker>
}

impl Slab {
    fn new() -> Self {
        Self { index: 0, cache: Vec::new(), wakers: Vec::new(), results: HashMap::new(), wakers_by_token: HashMap::new() }
    }

    fn add(&mut self, waker: std::task::Waker) -> u64 {
        if let Some(index) = self.cache.pop() {
            self.wakers[index as usize] = waker;
            index
        }else{
            let ret = self.index;
            self.wakers.push(waker);
            self.index += 1;
            ret
        }
    }

    pub fn add_by_token(&mut self, token: u64, waker: std::task::Waker) {
        self.wakers_by_token.insert(token, waker);
    }

    fn get_result(&self, index: u64) -> Option<i32> {
        self.results.get(&index).copied()
    }

    fn get_waker(&self, index: u64) -> Option<&std::task::Waker> {
        self.wakers.get(index as usize)
    }

    fn add_result(&mut self, index: u64, result: i32) {
        self.results.insert(index, result);
    }

    fn get_waker_by_token(&self, token: u64) -> Option<&std::task::Waker> {
        self.wakers_by_token.get(&token)
    }
}

use std::{cell::RefCell, rc::Rc};

pub struct TokenManager {
    token: u64
}

impl TokenManager {
    fn new() -> Self {
        Self { token: 1 }
    }

    pub fn get_token(&mut self) -> u64 {
        let token = self.token;
        self.token += 1;
        token
    }
}

thread_local! {
    pub static TASK_QUEUE : LazyLock<Rc<RefCell<Vec<(u64, AsyncTask)>>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Vec::new()))
    });
    pub static COMPLETION_QUEUE : LazyLock<Rc<RefCell<Vec<u64>>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Vec::new()))
    });
    pub static SLAB : LazyLock<Rc<RefCell<Slab>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Slab::new()))
    });
    pub static POLL : LazyLock<Rc<RefCell<Poll>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Poll::new()))
    });
    pub static TASKS : std::sync::LazyLock<Rc<RefCell<HashMap<u64, AsyncTask>>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(HashMap::new()))
    });
    pub static TOKEN_MANAGER : LazyLock<Rc<RefCell<TokenManager>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(TokenManager::new()))
    });

    pub static TIMER : LazyLock<Rc<RefCell<Timer>>> = LazyLock::new(|| {
        Rc::new(RefCell::new(Timer::new()))
    });
}

#[macro_export]
macro_rules! initialize_runtime {
    () => {
        let waker = $crate::Waker::new(0);
        let waker = std::task::Waker::from(Arc::new(waker));
        $crate::SLAB.with(|slab| {
            slab.borrow_mut().add_by_token(0, waker.clone());
        });
        let mut runtime = NoirRuntime::new(&waker);
        runtime.work();
    }
}

pub struct NoirRuntime<'a> {
    context: Context<'a>,
    c: Vec<std::task::Waker>
}

impl <'a> NoirRuntime<'a> {
    pub fn new(waker: &'a std::task::Waker) -> Self {
        Self { context: Context::from_waker(waker), c: Vec::new() }
    }

    pub fn work(&mut self) {
        let r = self.exec();
        r.iter().for_each(|x| {
            TASKS.with(|tasks| {
                println!("removing {x} from tasks");
                let mut tasks = tasks.borrow_mut();
                tasks.remove(x);
            });
        });
        loop {
            self.drain_queue();
            self.drain_completion();
            let Ok((ret, idx)) = POLL.with(|poll| {
                poll.borrow_mut().poll()
            }) else {
                continue
            };
            let is_timer = TIMER.with(|timer| {
                let mut timer = timer.borrow_mut();
                if !timer.is_timer(idx) { return false };
                timer.handle_event(ret);
                true
            });
            println!("poll result: {ret} {idx}");
            if is_timer {
                continue;
            }
            SLAB.with(|slab| {
                let mut slab = slab.borrow_mut();
                slab.add_result(idx, ret.into());
                let waker = slab.get_waker(idx).unwrap();
                waker.wake_by_ref();
            });
        }
    }

    fn exec_one(&self, task: &mut AsyncTask, waker: &std::task::Waker) -> bool {
        let mut context = Context::from_waker(waker);
        task.as_mut().poll(&mut context).is_ready()
    }

    fn drain_completion(&mut self) {
        COMPLETION_QUEUE.with(|queue| {
            let mut queue = queue.borrow_mut();
            while let Some(token) = queue.pop() {
                TASKS.with(|tasks| {
                    let mut tasks = tasks.borrow_mut();
                    let finished = {
                        let Some(task) = tasks.get_mut(&token) else {
                            return;
                        };
                        let waker = SLAB.with(|slab| {
                            let slab = slab.borrow();
                            slab.get_waker_by_token(token).unwrap().clone()
                        });
                        self.exec_one(task, &waker)
                    };
                    if finished {
                        tasks.remove(&token);
                    }
                });
            }
        });
    }

    fn drain_queue(&mut self) {
        TASKS.with(|tasks| {
            let mut tasks = tasks.borrow_mut();
            TASK_QUEUE.with(|queue| {
                let mut queue = queue.borrow_mut();
                while let Some((token, mut task)) = queue.pop() {
                    let waker = std::task::Waker::from(Arc::new(Waker::new(token)));
                    SLAB.with(|slab| {
                        slab.borrow_mut().add_by_token(token, waker.clone());
                    });
                    let mut context = Context::from_waker(&waker);
                    match task.as_mut().poll(&mut context) {
                        StdPoll::Pending => { tasks.insert(token, task); },
                        StdPoll::Ready(_) => ()
                    };
                    self.c.push(waker);
                }
            });
        });
    }

    pub fn exec(&mut self) -> Vec<u64> {
        let mut to_remove = vec![];
        TASKS.with(|tasks| {
            let mut tasks = tasks.borrow_mut();
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
                let mut queue = queue.borrow_mut();
                while let Some((token, mut task)) = queue.pop() {
                    let waker = std::task::Waker::from(Arc::new(Waker::new(token)));
                    SLAB.with(|slab| {
                        slab.borrow_mut().add_by_token(token, waker.clone());
                    });
                    let mut context = Context::from_waker(&waker);
                    match task.as_mut().poll(&mut context) {
                        StdPoll::Pending => {
                            println!("inserting task with token {token} to tasks");
                            tasks.insert(token, task);
                        },
                        StdPoll::Ready(_) => ()
                    };
                    self.c.push(waker);
                }
            });
        });
        to_remove
    }
}

#[macro_export]
macro_rules! push_task {
    ($task:expr) => {
        {
            let mut tasks = $crate::TASKS.with(|tasks| {
                let mut tasks = tasks.borrow_mut();
                let pin = Box::pin($task);
                tasks.insert(0, pin);
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
            let mut token = $crate::TOKEN_MANAGER.with(|manager| {
                manager.borrow_mut().get_token()
            });
            let mut tasks = $crate::TASK_QUEUE.with(|queue| {
                let mut queue = queue.borrow_mut();
                println!("SPAWN GOT LOCK");
                let pin = Box::pin($task);
                queue.push((token, pin));
            });
        }
    }
}
