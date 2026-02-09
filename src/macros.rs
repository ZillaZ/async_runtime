#[macro_export]
macro_rules! push_task {
    ($task:expr) => {
        {
            let mut tasks = $crate::__private::TASKS.with(|tasks| {
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
            let mut token = $crate::__private::TOKEN_MANAGER.with(|manager| {
                manager.borrow_mut().get_token()
            });
            let mut tasks = $crate::__private::TASK_QUEUE.with(|queue| {
                let mut queue = queue.borrow_mut();
                let pin = Box::pin($task);
                queue.push((token, pin));
            });
        }
    }
}

#[macro_export]
macro_rules! timeout {
    ($task:expr, $duration:expr) => {
        {
            use $crate::time::linked_timeout;
            use $crate::__private::Flags;
            $task.flags(Flags::IO_LINK);
            linked_timeout($duration).await;
            $task.await
        }
    }
}
