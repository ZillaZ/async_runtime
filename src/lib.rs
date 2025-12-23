pub(crate) mod runtime;
mod reactor;
mod descriptors;
pub mod net;
pub mod fs;
pub mod time;
pub mod io;

#[macro_use]
mod macros;

pub use runtime::initialize_runtime;

pub mod prelude {
    pub use crate::io::{AsyncRead, AsyncWrite, AsyncBufRead};
    pub use crate::spawn;
}

#[doc(hidden)]
pub mod __private {
    pub use crate::runtime::{TOKEN_MANAGER, TASK_QUEUE, TASKS};
}
