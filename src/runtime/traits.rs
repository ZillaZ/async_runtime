use io_uring::squeue::Flags;

pub trait IoUringTask {
    fn linked(&mut self, flags: Flags);
}
