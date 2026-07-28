//! The Win32 half of the Windows write confinement (ADR 0031): grant the RESTRICTED SID write access to
//! the policy's roots, build a restricted token, and spawn the child under it.
//!
//! This is the **only** file in the crate allowed to write `unsafe` (ADR 0032). There is no safe-Rust
//! path here: `CreateProcessAsUser` takes a token, and neither `std::os::windows::process::CommandExt`
//! nor `process-wrap` can attach one — which is also why this runs in a re-executed `kiri confined-exec`
//! child rather than in the adapter, keeping the single spawn site in `exec::run` untouched.
#![allow(unsafe_code)]

use std::ffi::{OsStr, OsString};
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use windows::Win32::Foundation::{CloseHandle, GENERIC_ALL, HANDLE};
use windows::Win32::Security::Authorization::{
    ACCESS_MODE, EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_WELL_KNOWN_GROUP, TRUSTEE_W,
};
use windows::Win32::Security::{
    ACL, CONTAINER_INHERIT_ACE, CopySid, CreateRestrictedToken, CreateWellKnownSid,
    DACL_SECURITY_INFORMATION, DISABLE_MAX_PRIVILEGE, GetTokenInformation, NO_INHERITANCE,
    OBJECT_INHERIT_ACE, PSID, SID_AND_ATTRIBUTES, SetTokenInformation, TOKEN_ALL_ACCESS,
    TOKEN_DEFAULT_DACL, TOKEN_GROUPS, TokenDefaultDacl, TokenGroups, WRITE_RESTRICTED,
    WinRestrictedCodeSid, WinWorldSid,
};
use windows::Win32::Storage::FileSystem::{
    DELETE, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE,
};
use windows::Win32::System::Console::{
    GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
};
use windows::Win32::System::Threading::{
    CREATE_UNICODE_ENVIRONMENT, CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess,
    INFINITE, OpenProcessToken, PROCESS_INFORMATION, STARTF_USESTDHANDLES, STARTUPINFOW,
    WaitForSingleObject,
};
use windows::core::PWSTR;

/// The `SE_GROUP_LOGON_ID` token-group attribute, which marks the logon-session SID. The `windows` crate
/// does not re-export it, so it is declared from the Win32 header value.
const SE_GROUP_LOGON_ID: u32 = 0xC000_0000;

/// A `SECURITY_MAX_SID_SIZE` buffer holding a SID — a well-known one, or the token's logon session.
///
/// Why this SID and not a Kiri-specific one: a restricting SID only confines if the objects the child
/// may touch carry an ACE for it, so *some* SID has to be written into the workspace's ACL either way.
/// A well-known one needs no allocation, no persistence, and no cleanup story — and Windows already
/// grants it read on the system directories, which is what lets a confined toolchain load its own DLLs.
/// The trade-off is real and documented in ADR 0031: any other restricted process on this machine can
/// write wherever Kiri granted RESTRICTED.
struct RestrictedSid {
    buffer: [u8; 68],
}

impl RestrictedSid {
    fn new() -> Result<Self, String> {
        Self::well_known(WinRestrictedCodeSid)
    }

    fn well_known(kind: windows::Win32::Security::WELL_KNOWN_SID_TYPE) -> Result<Self, String> {
        let mut sid = Self { buffer: [0u8; 68] };
        let mut length = sid.buffer.len() as u32;
        // SAFETY: `buffer` is `SECURITY_MAX_SID_SIZE` bytes and `length` describes it; the call writes at
        // most that many bytes and reports the actual size back.
        unsafe {
            CreateWellKnownSid(
                kind,
                None,
                Some(PSID(sid.buffer.as_mut_ptr().cast())),
                &mut length,
            )
        }
        .map_err(|error| format!("cannot build the RESTRICTED SID: {error}"))?;
        Ok(sid)
    }

    /// The logon-session SID (`S-1-5-5-x-y`) carried by `token`. A write-restricted token filters writes
    /// to the session's object namespace too, and `\Sessions\<n>\BaseNamedObjects` grants this SID rather
    /// than `Everyone` — without it `rustc` dies on `failed to create jobserver: Acesso negado`, because
    /// the named semaphore it opens per build lives there.
    fn logon_session(token: HANDLE) -> Result<Self, String> {
        let mut length = 0u32;
        // First call sizes the buffer; it reports the "insufficient buffer" error, which is expected.
        let _ = unsafe { GetTokenInformation(token, TokenGroups, None, 0, &mut length) };
        let mut buffer = vec![0u8; length as usize];
        unsafe {
            GetTokenInformation(
                token,
                TokenGroups,
                Some(buffer.as_mut_ptr().cast()),
                length,
                &mut length,
            )
        }
        .map_err(|error| format!("cannot read this process's token groups: {error}"))?;

        // SAFETY: the buffer was filled by the call above as a `TOKEN_GROUPS` whose `Groups` is a
        // variable-length array of `GroupCount` entries.
        let groups = unsafe { &*(buffer.as_ptr() as *const TOKEN_GROUPS) };
        let entries = unsafe {
            std::slice::from_raw_parts(groups.Groups.as_ptr(), groups.GroupCount as usize)
        };
        let session = entries
            .iter()
            .find(|group| group.Attributes & SE_GROUP_LOGON_ID != 0)
            .ok_or_else(|| "this process's token carries no logon-session SID".to_string())?;

        let mut sid = Self { buffer: [0u8; 68] };
        // SAFETY: the destination is `SECURITY_MAX_SID_SIZE` bytes, which bounds any SID.
        unsafe {
            CopySid(
                sid.buffer.len() as u32,
                PSID(sid.buffer.as_mut_ptr().cast()),
                session.Sid,
            )
        }
        .map_err(|error| format!("cannot copy the logon-session SID: {error}"))?;
        Ok(sid)
    }

    /// The pointer is valid for as long as `self` is: every consumer takes it through a borrow.
    fn psid(&self) -> PSID {
        PSID(self.buffer.as_ptr() as *mut _)
    }
}

/// Modify, matching what a build actually needs: read, write, execute, and delete its own outputs.
/// `WRITE_DAC`/`WRITE_OWNER` are withheld, so a confined command cannot grant itself more.
const MODIFY: u32 = FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0 | FILE_GENERIC_EXECUTE.0 | DELETE.0;

/// Add an inheritable ACE granting [`RestrictedSid`] Modify on `root`, so a child holding the restricting
/// SID can write there. `GRANT_ACCESS` merges with any ACE the trustee already has rather than appending a
/// duplicate, which is what makes calling this on every command idempotent.
///
/// **This is a persistent change to the user's filesystem** — the ACE outlives the process, the session,
/// and the install (ADR 0031). It is the price of the mechanism: without it the confined child cannot
/// write even the workspace.
fn grant_restricted_write(root: &Path, sid: &RestrictedSid) -> Result<(), String> {
    let trustee = TRUSTEE_W {
        TrusteeForm: TRUSTEE_IS_SID,
        TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
        ptstrName: PWSTR(sid.psid().0.cast()),
        ..Default::default()
    };
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: MODIFY,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: windows::Win32::Security::ACE_FLAGS(
            OBJECT_INHERIT_ACE.0 | CONTAINER_INHERIT_ACE.0,
        ),
        Trustee: trustee,
    };
    let mut new_acl: *mut ACL = std::ptr::null_mut();
    // SAFETY: `access` lives for the call; `new_acl` receives an allocation the OS owns and that
    // `SetNamedSecurityInfoW` copies out of before we drop the pointer.
    let status = unsafe { SetEntriesInAclW(Some(&[access]), None, &mut new_acl) };
    if status.is_err() {
        return Err(format!(
            "cannot build an ACL for {}: {status:?}",
            root.display()
        ));
    }
    let mut path = wide(root.as_os_str());
    // SAFETY: `path` is NUL-terminated and outlives the call; `new_acl` came from `SetEntriesInAclW`.
    let status = unsafe {
        SetNamedSecurityInfoW(
            PWSTR(path.as_mut_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(new_acl),
            None,
        )
    };
    if status.is_err() {
        return Err(format!(
            "cannot grant write access on {} ({status:?}); the harness must own the directory to \
             confine writes to it",
            root.display()
        ));
    }
    Ok(())
}

/// Give the restricted token a default DACL that grants [`RestrictedSid`], so every kernel object the
/// confined process creates carries an ACE the restricting-SID check can pass.
///
/// Without this a write-restricted process cannot use a pipe *it created itself*: the new object inherits
/// the token's default DACL, which grants the user SID and SYSTEM but none of the restricting SIDs, so the
/// second access check fails when the process opens the other end. That is what broke `rustc` spawning the
/// linker, `cargo`, and every shell pipeline — anything that captures a subprocess's output.
fn grant_self_access_to_new_objects(token: HANDLE, sid: &RestrictedSid) -> Result<(), String> {
    let mut length = 0u32;
    let _ = unsafe { GetTokenInformation(token, TokenDefaultDacl, None, 0, &mut length) };
    let mut buffer = vec![0u8; length as usize];
    unsafe {
        GetTokenInformation(
            token,
            TokenDefaultDacl,
            Some(buffer.as_mut_ptr().cast()),
            length,
            &mut length,
        )
    }
    .map_err(|error| format!("cannot read the restricted token's default DACL: {error}"))?;

    // SAFETY: the buffer was filled by the call above as a `TOKEN_DEFAULT_DACL`.
    let current = unsafe { &*(buffer.as_ptr() as *const TOKEN_DEFAULT_DACL) };
    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: GENERIC_ALL.0,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: NO_INHERITANCE,
        Trustee: TRUSTEE_W {
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_WELL_KNOWN_GROUP,
            ptstrName: PWSTR(sid.psid().0.cast()),
            ..Default::default()
        },
    };
    let mut merged: *mut ACL = std::ptr::null_mut();
    // Merges into the token's existing default DACL rather than replacing it, so the user SID and SYSTEM
    // keep the access they already had — the harness still needs to reach the objects it hands the child.
    let status =
        unsafe { SetEntriesInAclW(Some(&[access]), Some(current.DefaultDacl), &mut merged) };
    if status.is_err() {
        return Err(format!("cannot extend the default DACL: {status:?}"));
    }
    let replacement = TOKEN_DEFAULT_DACL {
        DefaultDacl: merged,
    };
    // SAFETY: `replacement` and the ACL it points at outlive the call, which copies the DACL into the token.
    unsafe {
        SetTokenInformation(
            token,
            TokenDefaultDacl,
            std::ptr::from_ref(&replacement).cast(),
            size_of::<TOKEN_DEFAULT_DACL>() as u32,
        )
    }
    .map_err(|error| format!("cannot set the restricted token's default DACL: {error}"))
}

fn wide(text: &OsStr) -> Vec<u16> {
    text.encode_wide().chain(Some(0)).collect()
}

/// Quote one argument the way `CommandLineToArgvW` parses it back: a backslash run immediately before a
/// quote (or before the closing quote) is doubled, an embedded quote is escaped, and the whole argument is
/// wrapped when it contains whitespace or a quote. `CreateProcessAsUserW` takes a single command line, so
/// the split the caller already made has to survive the round trip.
fn quote_argument(argument: &OsStr, out: &mut Vec<u16>) {
    let encoded: Vec<u16> = argument.encode_wide().collect();
    let needs_quotes = encoded.is_empty()
        || encoded
            .iter()
            .any(|unit| *unit == b' ' as u16 || *unit == b'\t' as u16 || *unit == b'"' as u16);
    if !needs_quotes {
        out.extend_from_slice(&encoded);
        return;
    }
    out.push(b'"' as u16);
    let mut backslashes = 0usize;
    for unit in encoded {
        if unit == b'\\' as u16 {
            backslashes += 1;
            continue;
        }
        if unit == b'"' as u16 {
            // Double the run, then escape the quote itself.
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2 + 1));
        } else {
            out.extend(std::iter::repeat_n(b'\\' as u16, backslashes));
        }
        backslashes = 0;
        out.push(unit);
    }
    // A trailing run would otherwise escape the closing quote.
    out.extend(std::iter::repeat_n(b'\\' as u16, backslashes * 2));
    out.push(b'"' as u16);
}

fn command_line(program: &OsStr, args: &[OsString]) -> Vec<u16> {
    let mut line = Vec::new();
    quote_argument(program, &mut line);
    for argument in args {
        line.push(b' ' as u16);
        quote_argument(argument, &mut line);
    }
    line.push(0);
    line
}

/// Whether this machine will let the harness confine writes to `root`: stamping the grant needs
/// `WRITE_DAC`, which only the directory's real owner has. A workspace on a secondary drive is the case
/// that fails — its ACL is inherited from a root owned by Administrators, and the user reaches it through
/// a group grant that carries no `WRITE_DAC`. Probed once at boot so the answer becomes one honest notice
/// instead of every command dying with the same error.
pub fn can_confine(root: &Path) -> Result<(), String> {
    let sid = RestrictedSid::new()?;
    grant_restricted_write(root, &sid)
}

/// Run `program args…` under a restricted token, inheriting this process's stdio, and return its exit
/// code. Every writable root is granted first: a root that does not exist is skipped (nothing to grant),
/// but one that exists and cannot be granted fails the whole call rather than letting the command run
/// somewhere it will mysteriously be unable to write.
pub fn run_confined(
    writable_roots: &[std::path::PathBuf],
    program: &OsStr,
    args: &[OsString],
) -> Result<u32, String> {
    let sid = RestrictedSid::new()?;
    for root in writable_roots {
        if root.exists() {
            grant_restricted_write(root, &sid)?;
        }
    }

    // SAFETY: each call below is checked; every pointer handed out lives in a local that outlives its
    // call, and every handle opened is closed on the way out.
    unsafe {
        let mut own = HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_ALL_ACCESS, &mut own)
            .map_err(|error| format!("cannot open this process's token: {error}"))?;

        // `Everyone` rides along in the restricting set because a write-restricted token filters *every*
        // write, including the ambient objects any process needs before it reaches user code: the NUL
        // device, the window station, the crypto provider's objects. Those grant Everyone, not RESTRICTED,
        // so a list holding RESTRICTED alone killed `git` on `/dev/null` and `pwsh` on BCrypt.dll init.
        // Files under the user's profile are granted to the user SID rather than Everyone, so they stay
        // blocked — which is the access this exists to stop.
        let everyone = RestrictedSid::well_known(WinWorldSid)?;
        let session = RestrictedSid::logon_session(own)?;
        let restrict = [
            SID_AND_ATTRIBUTES {
                Sid: sid.psid(),
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: everyone.psid(),
                Attributes: 0,
            },
            SID_AND_ATTRIBUTES {
                Sid: session.psid(),
                Attributes: 0,
            },
        ];
        let mut restricted = HANDLE::default();
        let created = CreateRestrictedToken(
            own,
            // WRITE_RESTRICTED is what makes this *write* confinement: without it the restricting SID is
            // evaluated on every access, so the child cannot read its own shell, the toolchain binaries,
            // or their DLLs, and `cargo --version` exits 1 with no output. DISABLE_MAX_PRIVILEGE drops
            // every privilege but SeChangeNotify, so the child cannot take ownership of a file to widen
            // its own access back out.
            DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED,
            None,
            None,
            Some(&restrict),
            &mut restricted,
        );
        let _ = CloseHandle(own);
        created.map_err(|error| format!("cannot build a restricted token: {error}"))?;
        if let Err(reason) = grant_self_access_to_new_objects(restricted, &sid) {
            let _ = CloseHandle(restricted);
            return Err(reason);
        }

        let startup = STARTUPINFOW {
            cb: size_of::<STARTUPINFOW>() as u32,
            dwFlags: STARTF_USESTDHANDLES,
            hStdInput: GetStdHandle(STD_INPUT_HANDLE).unwrap_or_default(),
            hStdOutput: GetStdHandle(STD_OUTPUT_HANDLE).unwrap_or_default(),
            hStdError: GetStdHandle(STD_ERROR_HANDLE).unwrap_or_default(),
            ..Default::default()
        };
        let mut info = PROCESS_INFORMATION::default();
        let mut line = command_line(program, args);
        let spawned = CreateProcessAsUserW(
            Some(restricted),
            None,
            Some(PWSTR(line.as_mut_ptr())),
            None,
            None,
            // The std handles above are only reachable by the child if it inherits them.
            true,
            CREATE_UNICODE_ENVIRONMENT,
            None,
            None,
            &startup,
            &mut info,
        );
        if let Err(error) = spawned {
            let _ = CloseHandle(restricted);
            return Err(format!("cannot spawn the confined command: {error}"));
        }

        WaitForSingleObject(info.hProcess, INFINITE);
        let mut code = 0u32;
        let read = GetExitCodeProcess(info.hProcess, &mut code);
        let _ = CloseHandle(info.hThread);
        let _ = CloseHandle(info.hProcess);
        let _ = CloseHandle(restricted);
        read.map_err(|error| format!("cannot read the confined command's exit code: {error}"))?;
        Ok(code)
    }
}

/// `SET_ACCESS` is imported so the `ACCESS_MODE` constants stay visible next to the one this uses; the
/// grant mode is deliberate and reviewed, so name its alternative rather than leaving it to memory.
const _: ACCESS_MODE = SET_ACCESS;

#[cfg(test)]
mod tests {
    use super::*;

    fn line(program: &str, args: &[&str]) -> String {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        let encoded = command_line(OsStr::new(program), &args);
        String::from_utf16_lossy(&encoded[..encoded.len() - 1])
    }

    #[test]
    fn a_plain_argument_is_not_quoted() {
        assert_eq!(line("pwsh", &["-Command", "ls"]), "pwsh -Command ls");
    }

    #[test]
    fn whitespace_and_quotes_are_wrapped_and_escaped() {
        // The shell script `run_shell` passes is one argument containing spaces and often quotes; if the
        // round trip through the command line splits it, the confined child runs a different command.
        assert_eq!(line("pwsh", &["a b"]), "pwsh \"a b\"");
        assert_eq!(line("pwsh", &["say \"hi\""]), "pwsh \"say \\\"hi\\\"\"");
    }

    #[test]
    fn a_trailing_backslash_cannot_escape_the_closing_quote() {
        // `C:\dir\` as a quoted argument must not become `"C:\dir\"`, which would swallow the delimiter
        // and merge this argument with the next.
        assert_eq!(
            line("x", &["C:\\dir with space\\"]),
            "x \"C:\\dir with space\\\\\""
        );
    }

    #[test]
    fn an_empty_argument_survives_as_an_empty_pair_of_quotes() {
        assert_eq!(line("x", &[""]), "x \"\"");
    }

    #[test]
    fn the_restricted_sid_builds() {
        assert!(RestrictedSid::new().is_ok());
    }
}
