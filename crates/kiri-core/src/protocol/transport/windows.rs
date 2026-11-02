use std::ffi::OsString;
use std::fs::{self, File};
use std::io;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::path::{Path, PathBuf};
use std::ptr;

use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, ServerOptions,
};
use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_SUCCESS, HANDLE, LocalFree,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertSidToStringSidW, ConvertStringSecurityDescriptorToSecurityDescriptorW, GetSecurityInfo,
    SDDL_REVISION_1, SE_KERNEL_OBJECT,
};
use windows_sys::Win32::Security::{
    GetTokenInformation, OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID,
    SECURITY_ATTRIBUTES, TOKEN_QUERY, TOKEN_USER, TokenUser,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

use crate::protocol::{ConnectError, Endpoint, EndpointError};

pub(crate) type ClientStream = NamedPipeClient;

pub(crate) fn address(_kiri_dir: &Path) -> Result<PathBuf, EndpointError> {
    let sid = owner_sid().map_err(EndpointError::NoOwner)?;
    Ok(PathBuf::from(format!(r"\\.\pipe\kiri-{sid}")))
}

pub(crate) fn secure_directory(path: &Path) -> io::Result<()> {
    fs::create_dir_all(path)
}

pub(crate) fn restrict_file(_file: &File) -> io::Result<()> {
    Ok(())
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
    // SAFETY: `user` points at the TOKEN_USER written above, alive during this call.
    sid_string(unsafe { (*user).User.Sid })
}

fn sid_string(sid: PSID) -> io::Result<String> {
    let mut wide: *mut u16 = ptr::null_mut();
    // SAFETY: the caller passes a valid SID, alive during this call.
    if unsafe { ConvertSidToStringSidW(sid, &mut wide) } == 0 {
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

fn pipe_owner_sid(client: &NamedPipeClient) -> io::Result<String> {
    let mut owner: PSID = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: the handle is open while `client` is borrowed, and the out pointers are valid or null.
    let status = unsafe {
        GetSecurityInfo(
            client.as_raw_handle(),
            SE_KERNEL_OBJECT,
            OWNER_SECURITY_INFORMATION,
            &mut owner,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null_mut(),
            &mut descriptor,
        )
    };
    if status != ERROR_SUCCESS {
        return Err(io::Error::from_raw_os_error(status as i32));
    }
    let sid = sid_string(owner);
    // SAFETY: `descriptor` was allocated by GetSecurityInfo, holds `owner`, and is freed once, after its last use.
    unsafe { LocalFree(descriptor) };
    sid
}

pub(crate) async fn connect(endpoint: &Endpoint) -> Result<ClientStream, ConnectError> {
    let client = ClientOptions::new()
        .open(endpoint.address())
        .map_err(|cause| match cause.raw_os_error() {
            Some(code) if code == ERROR_ACCESS_DENIED as i32 => ConnectError::ForeignPipe,
            _ => cause.into(),
        })?;
    if pipe_owner_sid(&client)? != owner_sid()? {
        return Err(ConnectError::ForeignPipe);
    }
    Ok(client)
}

struct OwnerOnlyDescriptor(PSECURITY_DESCRIPTOR);

impl OwnerOnlyDescriptor {
    fn new() -> io::Result<Self> {
        let sid = owner_sid()?;
        let definition: Vec<u16> = format!("O:{sid}D:P(A;;GA;;;{sid})\0")
            .encode_utf16()
            .collect();
        let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
        // SAFETY: `definition` is NUL-terminated, the out pointer is valid, and the length out pointer may be null.
        if unsafe {
            ConvertStringSecurityDescriptorToSecurityDescriptorW(
                definition.as_ptr(),
                SDDL_REVISION_1,
                &mut descriptor,
                ptr::null_mut(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        Ok(Self(descriptor))
    }

    fn create_pipe(&self, name: &Path, first: bool) -> io::Result<NamedPipeServer> {
        let mut attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: self.0,
            bInheritHandle: 0,
        };
        // SAFETY: `attributes` and the descriptor it points at are alive during this call.
        unsafe {
            ServerOptions::new()
                .first_pipe_instance(first)
                .reject_remote_clients(true)
                .create_with_security_attributes_raw(name, (&raw mut attributes).cast())
        }
    }
}

impl Drop for OwnerOnlyDescriptor {
    fn drop(&mut self) {
        // SAFETY: the OS allocated the descriptor in `new`, and it is freed once.
        unsafe { LocalFree(self.0) };
    }
}

pub(crate) struct Listener {
    name: PathBuf,
    descriptor: OwnerOnlyDescriptor,
    next: NamedPipeServer,
}

impl Listener {
    pub(crate) fn bind(endpoint: &Endpoint) -> io::Result<Self> {
        let name = endpoint.address().to_path_buf();
        let descriptor = OwnerOnlyDescriptor::new()?;
        let next = descriptor.create_pipe(&name, true)?;
        Ok(Self {
            name,
            descriptor,
            next,
        })
    }

    pub(crate) async fn accept(&mut self) -> io::Result<NamedPipeServer> {
        self.next.connect().await?;
        let next = self.descriptor.create_pipe(&self.name, false)?;
        Ok(std::mem::replace(&mut self.next, next))
    }
}
