use std::fs::File;
use std::hash::{BuildHasher, RandomState};
use std::io;
use std::path::Path;
use std::process::{Command, Stdio};
use std::ptr;

use windows_sys::Win32::Foundation::{
    CloseHandle, ERROR_ACCESS_DENIED, ERROR_BROKEN_PIPE, ERROR_PIPE_LISTENING, HANDLE,
    INVALID_HANDLE_VALUE, LocalFree,
};
use windows_sys::Win32::NetworkManagement::NetManagement::{
    NetUserAdd, NetUserDel, UF_DONT_EXPIRE_PASSWD, UF_SCRIPT, USER_INFO_1, USER_PRIV_USER,
};
use windows_sys::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows_sys::Win32::Security::{
    ImpersonateLoggedOnUser, LOGON32_LOGON_INTERACTIVE, LOGON32_PROVIDER_DEFAULT, LogonUserW,
    PSECURITY_DESCRIPTOR, RevertToSelf, SECURITY_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::PIPE_ACCESS_DUPLEX;
use windows_sys::Win32::System::Pipes::{CreateNamedPipeW, PIPE_TYPE_BYTE, PeekNamedPipe};

use crate::{Outcome, OwnerClient, TemporaryHome, ensure};

const REFUSAL: &str = "The Kiri daemon pipe belongs to another user. Refusing to connect.";
const OPEN_TO_EVERYONE: &str = "D:(A;;GA;;;WD)";
const OPEN_ONLY_TO_ITS_OWNER: &str = "D:(A;;GA;;;OW)";

fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain([0]).collect()
}

fn generated_password() -> String {
    let [first, second] = [(); 2].map(|()| RandomState::new().hash_one(0u8));
    format!("Aa1!{first:016x}{second:016x}")
}

struct TemporaryAccount {
    name: Vec<u16>,
}

impl TemporaryAccount {
    fn create(password: &[u16]) -> Outcome<Self> {
        let mut name = wide(&format!("kiri-probe-{}", std::process::id()));
        let mut password = password.to_vec();
        let info = USER_INFO_1 {
            usri1_name: name.as_mut_ptr(),
            usri1_password: password.as_mut_ptr(),
            usri1_password_age: 0,
            usri1_priv: USER_PRIV_USER,
            usri1_home_dir: ptr::null_mut(),
            usri1_comment: ptr::null_mut(),
            usri1_flags: UF_SCRIPT | UF_DONT_EXPIRE_PASSWD,
            usri1_script_path: ptr::null_mut(),
        };
        // SAFETY: `info` and the strings it points at are alive during this call.
        let status =
            unsafe { NetUserAdd(ptr::null(), 1, (&raw const info).cast(), ptr::null_mut()) };
        ensure(status == 0, format!("NetUserAdd failed with {status}"))?;
        Ok(Self { name })
    }

    fn log_on(&self, password: &[u16]) -> Outcome<Token> {
        let mut token: HANDLE = ptr::null_mut();
        let local_machine = wide(".");
        // SAFETY: the three strings are NUL-terminated and the out pointer is valid.
        if unsafe {
            LogonUserW(
                self.name.as_ptr(),
                local_machine.as_ptr(),
                password.as_ptr(),
                LOGON32_LOGON_INTERACTIVE,
                LOGON32_PROVIDER_DEFAULT,
                &mut token,
            )
        } == 0
        {
            return Err(format!("LogonUserW failed: {}", io::Error::last_os_error()).into());
        }
        Ok(Token(token))
    }
}

impl Drop for TemporaryAccount {
    fn drop(&mut self) {
        // SAFETY: both pointers are a null server name and this account's NUL-terminated name.
        let status = unsafe { NetUserDel(ptr::null(), self.name.as_ptr()) };
        if status != 0 {
            eprintln!("NetUserDel failed with {status}");
        }
    }
}

struct Token(HANDLE);

impl Token {
    fn run_as_user<T>(&self, action: impl FnOnce() -> T) -> Outcome<T> {
        // SAFETY: the token comes from LogonUserW and stays open while `self` lives.
        if unsafe { ImpersonateLoggedOnUser(self.0) } == 0 {
            return Err(format!(
                "ImpersonateLoggedOnUser failed: {}",
                io::Error::last_os_error()
            )
            .into());
        }
        let result = action();
        // SAFETY: this thread is impersonating, and reverting takes no arguments.
        assert_ne!(unsafe { RevertToSelf() }, 0, "RevertToSelf failed");
        Ok(result)
    }
}

impl Drop for Token {
    fn drop(&mut self) {
        // SAFETY: the token is open and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

struct ForeignPipe(HANDLE);

impl ForeignPipe {
    fn create(token: &Token, name: &str, access: &str) -> Outcome<Self> {
        let definition = wide(access);
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
            return Err(io::Error::last_os_error().into());
        }
        let attributes = SECURITY_ATTRIBUTES {
            nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
            lpSecurityDescriptor: descriptor,
            bInheritHandle: 0,
        };
        let name = wide(name);
        let created = token.run_as_user(|| {
            // SAFETY: `name` is NUL-terminated, and `attributes` and its descriptor outlive this call.
            let handle = unsafe {
                CreateNamedPipeW(
                    name.as_ptr(),
                    PIPE_ACCESS_DUPLEX,
                    PIPE_TYPE_BYTE,
                    1,
                    4096,
                    4096,
                    0,
                    &raw const attributes,
                )
            };
            (handle, io::Error::last_os_error())
        });
        // SAFETY: the OS allocated `descriptor`, and it is freed once, after its last use.
        unsafe { LocalFree(descriptor) };
        let (handle, cause) = created?;
        ensure(
            handle != INVALID_HANDLE_VALUE,
            format!("CreateNamedPipeW failed: {cause}"),
        )?;
        Ok(Self(handle))
    }

    fn holds_no_bytes(&self) -> Outcome<bool> {
        let mut available = 0;
        // SAFETY: the pipe is open, the other out pointers may be null, and `available` is valid.
        let peeked = unsafe {
            PeekNamedPipe(
                self.0,
                ptr::null_mut(),
                0,
                ptr::null_mut(),
                &mut available,
                ptr::null_mut(),
            )
        };
        if peeked != 0 {
            return Ok(available == 0);
        }
        let cause = io::Error::last_os_error();
        match cause.raw_os_error().map(|code| code as u32) {
            Some(ERROR_BROKEN_PIPE | ERROR_PIPE_LISTENING) => Ok(true),
            _ => Err(format!("PeekNamedPipe failed: {cause}").into()),
        }
    }
}

impl Drop for ForeignPipe {
    fn drop(&mut self) {
        // SAFETY: the pipe is open and is closed once.
        unsafe { CloseHandle(self.0) };
    }
}

fn own_sid() -> Outcome<String> {
    let output = Command::new("whoami")
        .args(["/user", "/fo", "csv", "/nh"])
        .output()?;
    ensure(output.status.success(), "whoami failed")?;
    let line = String::from_utf8(output.stdout)?;
    let sid = line
        .trim()
        .rsplit(',')
        .next()
        .ok_or("whoami printed no SID")?;
    Ok(sid.trim_matches('"').to_owned())
}

fn temporary_home(label: &str) -> Outcome<TemporaryHome> {
    TemporaryHome::create(
        std::env::temp_dir().join(format!("kiri-cross-user-{label}-{}", std::process::id())),
    )
}

fn expect_denied(token: &Token, pipe: &str, write: bool) -> Outcome {
    let opened = token.run_as_user(|| File::options().read(true).write(write).open(pipe))?;
    let Err(cause) = opened else {
        return Err(format!("the second user opened the pipe with write={write}").into());
    };
    ensure(
        cause.raw_os_error() == Some(ERROR_ACCESS_DENIED as i32),
        format!("expected access denied with write={write}, got: {cause}"),
    )
}

fn expect_refusal(kiri: &Path, home: &TemporaryHome, pipe: &ForeignPipe) -> Outcome {
    let output = home.kiri(kiri).stdin(Stdio::null()).output()?;
    let stderr = String::from_utf8_lossy(&output.stderr);
    ensure(
        output.status.code() == Some(1) && stderr.trim_end() == REFUSAL,
        format!("kiri ended with {}, printing: {stderr}", output.status),
    )?;
    ensure(
        pipe.holds_no_bytes()?,
        "kiri sent bytes to the foreign pipe",
    )?;
    ensure(!home.log_path().exists(), "kiri started a daemon")
}

pub fn cross_user(kiri: &Path) -> Outcome {
    let password = wide(&generated_password());
    println!("Creating a temporary account.");
    let account = TemporaryAccount::create(&password)?;
    let token = account.log_on(&password)?;
    let pipe = format!(r"\\.\pipe\kiri-{}", own_sid()?);

    println!("Scenario 8: another OS user is refused.");
    {
        let home = temporary_home("denied")?;
        let _client = OwnerClient::start(kiri, &home)?;
        expect_denied(&token, &pipe, true)?;
        expect_denied(&token, &pipe, false)?;
    }

    println!("Scenario 10: a pipe owned by another user is refused.");
    {
        let home = temporary_home("foreign")?;
        let foreign = ForeignPipe::create(&token, &pipe, OPEN_TO_EVERYONE)?;
        expect_refusal(kiri, &home, &foreign)?;
    }

    println!("A foreign pipe that denies access is refused the same way.");
    let home = temporary_home("foreign-denied")?;
    let foreign = ForeignPipe::create(&token, &pipe, OPEN_ONLY_TO_ITS_OWNER)?;
    expect_refusal(kiri, &home, &foreign)?;
    Ok(())
}
