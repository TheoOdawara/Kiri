use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

static ONE_DAEMON_AT_A_TIME: Mutex<()> = Mutex::new(());
static NEXT_HOME: AtomicU32 = AtomicU32::new(0);

struct TestHome {
    path: PathBuf,
    _turn: MutexGuard<'static, ()>,
}

impl TestHome {
    fn new() -> Self {
        let turn = ONE_DAEMON_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let unique = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("k-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self { path, _turn: turn }
    }

    fn kiri(&self, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiri"));
        command
            .args(arguments)
            .env("HOME", &self.path)
            .env("USERPROFILE", &self.path)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    fn kiri_dir(&self) -> PathBuf {
        self.path.join(".kiri")
    }

    fn log_path(&self) -> PathBuf {
        self.kiri_dir().join("logs").join("daemon.jsonl")
    }

    fn log(&self) -> String {
        fs::read_to_string(self.log_path()).unwrap_or_default()
    }

    fn started_daemons(&self) -> usize {
        self.log().matches("daemon started").count()
    }

    fn wait_until(&self, condition: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !condition() {
            assert!(Instant::now() < deadline, "condition not met in 10 seconds");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    fn wait_for_daemon_exit(&self) {
        let lock_path = self.kiri_dir().join("daemon.lock");
        self.wait_until(|| {
            File::options()
                .write(true)
                .open(&lock_path)
                .is_ok_and(|lock| lock.try_lock().is_ok())
        });
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn wait_for_input_prompt(client: &mut Child) {
    let stdout: ChildStdout = client.stdout.take().unwrap();
    let mut prompt = [0; 2];
    BufReader::new(stdout).read_exact(&mut prompt).unwrap();
    assert_eq!(&prompt, b"> ");
}

fn end_client(mut client: Child) {
    drop(client.stdin.take());
    assert_eq!(client.wait().unwrap().code(), Some(0));
}

#[test]
fn two_clients_starting_together_share_one_daemon() {
    let home = TestHome::new();
    let mut first = home.kiri(&[]).spawn().unwrap();
    let mut second = home.kiri(&[]).spawn().unwrap();

    wait_for_input_prompt(&mut first);
    wait_for_input_prompt(&mut second);
    assert_eq!(home.started_daemons(), 1);

    end_client(first);
    end_client(second);
    home.wait_for_daemon_exit();
    assert!(home.log().contains("daemon exited"));
}

#[test]
fn a_killed_daemon_is_replaced_at_the_next_start() {
    let home = TestHome::new();
    let mut killed = home.kiri(&["daemon"]).spawn().unwrap();
    home.wait_until(|| home.started_daemons() == 1);
    killed.kill().unwrap();
    killed.wait().unwrap();
    #[cfg(unix)]
    assert!(home.kiri_dir().join("daemon.sock").exists());

    let mut client = home.kiri(&[]).spawn().unwrap();
    wait_for_input_prompt(&mut client);
    assert_eq!(home.started_daemons(), 2);

    end_client(client);
    home.wait_for_daemon_exit();
}

#[test]
fn a_daemon_no_client_reaches_exits_after_five_seconds() {
    let home = TestHome::new();
    let started_at = Instant::now();

    let output = home.kiri(&["daemon"]).output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert!(started_at.elapsed() >= Duration::from_secs(5));
}

#[test]
fn a_daemon_that_cannot_start_times_out_the_client_and_logs_the_cause() {
    let home = TestHome::new();
    fs::create_dir_all(home.kiri_dir().join("daemon.lock")).unwrap();
    let started_at = Instant::now();

    let output = home.kiri(&[]).output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert!(started_at.elapsed() < Duration::from_secs(7));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim_end(),
        format!(
            "Could not connect to the Kiri daemon within 5 seconds. See {}.",
            home.log_path().display()
        )
    );
    assert!(home.log().contains("could not take the daemon lock"));
}

#[cfg(unix)]
#[test]
fn a_home_too_long_for_a_unix_socket_stops_the_client() {
    let home = TestHome::new();
    let long_home = home.path.join("k".repeat(120));
    let socket = long_home.join(".kiri").join("daemon.sock");
    let limit = if cfg!(target_os = "macos") { 104 } else { 108 };

    let output = home.kiri(&[]).env("HOME", &long_home).output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim_end(),
        format!(
            "The path {} is too long for a Unix socket. The limit on this system is {limit} bytes.",
            socket.display()
        )
    );
}

#[test]
fn a_second_daemon_by_hand_reports_the_running_one() {
    let home = TestHome::new();
    let mut client = home.kiri(&[]).spawn().unwrap();
    wait_for_input_prompt(&mut client);

    let output = home.kiri(&["daemon"]).output().unwrap();

    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        String::from_utf8_lossy(&output.stderr).trim_end(),
        "A Kiri daemon is already running."
    );

    end_client(client);
    home.wait_for_daemon_exit();
}
