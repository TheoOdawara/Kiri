use std::fs::{self, Permissions};
use std::io::{ErrorKind, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use crate::{Outcome, OwnerClient, TemporaryHome, ensure, wait_until};

const DENIED_AT_CONNECT: u8 = 3;
const ANSWERED: u8 = 4;
const CLOSED_WITHOUT_ANSWER: u8 = 5;
const STAYED_OPEN: u8 = 6;
const COULD_NOT_CONNECT: u8 = 7;
const INITIALIZE: &[u8] = b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"protocolVersion\":1}}\n";
const REFUSAL: &str = "refused a connection from another user";
const DAEMON_LIFETIME_WITHOUT_A_CLIENT: Duration = Duration::from_secs(8);

pub fn probe(socket: &str) -> ExitCode {
    ExitCode::from(probe_outcome(socket))
}

fn probe_outcome(socket: &str) -> u8 {
    let mut stream = match UnixStream::connect(socket) {
        Ok(stream) => stream,
        Err(cause) if cause.kind() == ErrorKind::PermissionDenied => return DENIED_AT_CONNECT,
        Err(_) => return COULD_NOT_CONNECT,
    };
    let _ = stream.write_all(INITIALIZE);
    if stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .is_err()
    {
        return COULD_NOT_CONNECT;
    }
    let mut first_byte = [0];
    match stream.read(&mut first_byte) {
        Ok(0) => CLOSED_WITHOUT_ANSWER,
        Ok(_) => ANSWERED,
        Err(cause) if matches!(cause.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {
            STAYED_OPEN
        }
        Err(_) => CLOSED_WITHOUT_ANSWER,
    }
}

struct Probe {
    executable: PathBuf,
    socket: PathBuf,
}

impl Probe {
    fn expect(&self, expected: u8) -> Outcome {
        let status = Command::new("sudo")
            .args(["-n", "-u", "nobody"])
            .arg(&self.executable)
            .arg("probe")
            .arg(&self.socket)
            .current_dir("/")
            .stdin(Stdio::null())
            .status()?;
        ensure(
            status.code() == Some(i32::from(expected)),
            format!("the second user's probe ended with {status}, expected code {expected}"),
        )
    }
}

fn log_count(home: &TemporaryHome, line: &str) -> usize {
    fs::read_to_string(home.log_path())
        .unwrap_or_default()
        .matches(line)
        .count()
}

fn mode_of(path: &Path) -> Outcome<u32> {
    Ok(fs::metadata(path)?.permissions().mode() & 0o777)
}

fn widen_permissions(home: &TemporaryHome) -> Outcome {
    let world = Permissions::from_mode(0o777);
    fs::set_permissions(home.kiri_dir(), world.clone())?;
    fs::set_permissions(home.kiri_dir().join("daemon.sock"), world)?;
    Ok(())
}

pub fn cross_user(kiri: &Path) -> Outcome {
    let home = TemporaryHome::create(PathBuf::from(format!(
        "/tmp/kiri-cross-user-{}",
        std::process::id()
    )))?;
    fs::set_permissions(&home.0, Permissions::from_mode(0o755))?;
    let probe = Probe {
        executable: home.0.join("probe"),
        socket: home.kiri_dir().join("daemon.sock"),
    };
    fs::copy(std::env::current_exe()?, &probe.executable)?;
    fs::set_permissions(&probe.executable, Permissions::from_mode(0o755))?;

    println!("Scenario 8: another OS user is refused.");
    let client = OwnerClient::start(kiri, &home)?;
    ensure(mode_of(&home.kiri_dir())? == 0o700, "~/.kiri is not 0700")?;
    ensure(mode_of(&probe.socket)? == 0o600, "the socket is not 0600")?;
    probe.expect(DENIED_AT_CONNECT)?;
    ensure(
        log_count(&home, REFUSAL) == 0,
        "a refusal was logged too early",
    )?;

    println!("Scenario 9: the peer check holds when permissions are widened.");
    widen_permissions(&home)?;
    probe.expect(CLOSED_WITHOUT_ANSWER)?;
    wait_until(Duration::from_secs(5), "the refusal in the log", || {
        log_count(&home, REFUSAL) == 1
    })?;
    drop(client);

    println!("A refused connection does not keep the daemon alive.");
    let started_at = Instant::now();
    let mut daemon = home
        .kiri(kiri)
        .arg("daemon")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    wait_until(Duration::from_secs(4), "the second daemon to start", || {
        log_count(&home, "daemon started") == 2
    })?;
    widen_permissions(&home)?;
    probe.expect(CLOSED_WITHOUT_ANSWER)?;
    ensure(
        log_count(&home, REFUSAL) == 2,
        "the second refusal is not in the log",
    )?;
    while daemon.try_wait()?.is_none() {
        ensure(
            started_at.elapsed() < DAEMON_LIFETIME_WITHOUT_A_CLIENT,
            "a daemon with only a refused connection did not exit",
        )?;
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}
