#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

#[cfg(unix)]
pub(crate) use unix::{ClientStream, Listener, address, connect};
#[cfg(windows)]
pub(crate) use windows::{ClientStream, Listener, address, connect};
