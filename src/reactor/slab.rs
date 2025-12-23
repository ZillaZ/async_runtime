pub struct TokenManager {
    token: u64,
    cache: Vec<u64>
}

impl TokenManager {
    pub fn new() -> Self {
        Self { token: 1, cache: Vec::new() }
    }

    pub fn get_token(&mut self) -> u64 {
        if let Some(token) = self.cache.pop() {
            token
        }else{
            let token = self.token;
            self.token += 1;
            token
        }
    }

    pub fn clear_token(&mut self, token: u64) {
        self.cache.push(token);
    }
}

use std::collections::HashMap;
use uring_lib::{read_cq, write_sq, setup_rings, setup_io_uring};

#[repr(C)]
#[repr(packed)]
#[derive(Clone, Copy)]
pub struct Index {
    pub index: u32,
    pub generation: u32
}

pub struct Slab {
    index: u32,
    cache: Vec<Index>,
    wakers: Vec<Option<(u32, std::task::Waker)>>,
    results: Vec<i32>,
    wakers_by_token: HashMap<u64, std::task::Waker>
}

impl Slab {
    pub fn new() -> Self {
        Self { index: 0, cache: Vec::new(), wakers: Vec::new(), results: Vec::new(), wakers_by_token: HashMap::new() }
    }

    pub fn add(&mut self, waker: std::task::Waker) -> Index {
        let Some(mut index) = self.cache.pop() else {
            return self.new_index(waker);
        };
        if index.generation == u32::MAX {
            self.new_index(waker)
        }else{
            let (i, g) = (index.index, index.generation);
            println!("USING CACHED INDEX {} gen {}", i, g);
            index.generation = index.generation.wrapping_add(1);
            self.wakers[index.index as usize] = Some((index.generation, waker));
            self.results[index.index as usize] = 0;
            index
        }
    }

    pub fn new_index(&mut self, waker: std::task::Waker) -> Index {
        println!("CREATING NEW INDEX");
        let ret = self.index;
        self.wakers.push(Some((0, waker)));
        self.results.push(0);
        self.index += 1;
        Index { index: ret, generation: 0 }
    }

    pub fn add_by_token(&mut self, token: u64, waker: std::task::Waker) {
        if self.wakers_by_token.contains_key(&token) {
            println!("ALREADY HAD A KEY");
        }
        self.wakers_by_token.insert(token, waker);
    }

    pub fn get_result(&mut self, index: Index) -> Option<i32> {
        self.results.get(index.index as usize).copied()
    }

    pub fn get_waker(&self, index: Index) -> Option<&std::task::Waker> {
        let Some((generation, waker)) = self.wakers.get(index.index as usize).unwrap() else {
            return None;
        };
        if *generation != index.generation {
            None
        }else{
            Some(waker)
        }
    }

    pub fn add_result(&mut self, index: Index, result: i32) {
        let Some((generation, _)) = self.wakers.get(index.index as usize).unwrap() else {
            return;
        };
        if *generation != index.generation {
            return;
        }
        self.results[index.index as usize] = result;
    }

    pub fn get_waker_by_token(&self, token: u64) -> Option<&std::task::Waker> {
        self.wakers_by_token.get(&token)
    }

    pub fn clear_index(&mut self, index: Index) {
        self.wakers[index.index as usize] = None;
        self.cache.push(index);
    }

    pub fn clear_token(&mut self, token: u64) {
        self.wakers_by_token.remove(&token);
    }
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
