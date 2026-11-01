use std::ffi::OsString;
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};
use std::ptr;

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
use windows_sys::Win32::Security::Authorization::ConvertSidToStringSidW;
use windows_sys::Win32::Security::{GetTokenInformation, TOKEN_QUERY, TOKEN_USER, TokenUser};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::protocol::{Endpoint, EndpointError};

pub(crate) type ClientStream = NamedPipeClient;

pub(crate) fn address(_kiri_dir: &Path) -> Result<PathBuf, EndpointError> {
    let sid = owner_sid().map_err(EndpointError::NoOwner)?;
    Ok(PathBuf::from(format!(r"\\.\pipe\kiri-{sid}")))
}

fn owner_sid() -> io::Result<String> {
    let mut token: HANDLE = ptr::null_mut();
    // SAFETY: the pseudo handle of the current process is always valid and `token` is a valid out pointer.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let sid = token_user_sid(token);
    // SAFETY: `token` was opened above and is closed once.
    unsafe { CloseHandle(token) };
    sid
}

fn token_user_sid(token: HANDLE) -> io::Result<String> {
    let mut needed = 0;
    // SAFETY: a null buffer of length zero only asks for the required length, written to `needed`.
    unsafe { GetTokenInformation(token, TokenUser, ptr::null_mut(), 0, &mut needed) };
    let mut buffer = vec![0u64; (needed as usize).div_ceil(size_of::<u64>())];
    // SAFETY: `buffer` holds at least `needed` bytes and u64 alignment satisfies TOKEN_USER.
    if unsafe {
        GetTokenInformation(
            token,
            TokenUser,
            buffer.as_mut_ptr().cast(),
            needed,
            &mut needed,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let user = buffer.as_ptr().cast::<TOKEN_USER>();
    let mut wide: *mut u16 = ptr::null_mut();
    // SAFETY: `user` points at the TOKEN_USER written above, alive during this call.
    if unsafe { ConvertSidToStringSidW((*user).User.Sid, &mut wide) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: on success `wide` is a NUL-terminated string the OS allocated, freed once with LocalFree.
    let sid = unsafe {
        let length = (0..).take_while(|&offset| *wide.add(offset) != 0).count();
        let sid = OsString::from_wide(std::slice::from_raw_parts(wide, length));
        LocalFree(wide.cast());
        sid
    };
    Ok(sid.to_string_lossy().into_owned())
}

pub(crate) async fn connect(endpoint: &Endpoint) -> io::Result<ClientStream> {
    ClientOptions::new().open(endpoint.address())
}

pub(crate) struct Listener {
    name: PathBuf,
    next: NamedPipeServer,
}

impl Listener {
    pub(crate) fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        let name = endpoint.address().to_path_buf();
        let next = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&name)?;
        Ok(Self { name, next })
    }

    pub(crate) async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        self.next.connect().await?;
        let next = ServerOptions::new().create(&self.name)?;
        Ok(std::mem::replace(&mut self.next, next))
    }
}
