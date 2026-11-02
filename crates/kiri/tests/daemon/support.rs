use std::fs::{self, File};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

static ONE_DAEMON_AT_A_TIME: Mutex<()> = Mutex::new(());
static NEXT_HOME: AtomicU32 = AtomicU32::new(0);
const READ_TIMEOUT: Duration = Duration::from_secs(10);

const KIRI_VARIABLES: [&str; 6] = [
    "KIRI_BASE_URL",
    "KIRI_API_KEY",
    "KIRI_MODEL",
    "KIRI_REASONING_EFFORT",
    "KIRI_THINKING",
    "KIRI_REQUEST_PARAMS",
];

pub struct TestHome {
    pub path: PathBuf,
    _turn: MutexGuard<'static, ()>,
}

impl TestHome {
    pub fn new() -> Self {
        let turn = ONE_DAEMON_AT_A_TIME
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let unique = NEXT_HOME.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("k-{}-{unique}", std::process::id()));
        fs::create_dir_all(&path).unwrap();
        Self { path, _turn: turn }
    }

    pub fn kiri(&self, arguments: &[&str]) -> Command {
        self.kiri_on("http://127.0.0.1:1", arguments)
    }

    pub fn kiri_on(&self, base_url: &str, arguments: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_kiri"));
        for variable in KIRI_VARIABLES {
            command.env_remove(variable);
        }
        command
            .args(arguments)
            .env("HOME", &self.path)
            .env("USERPROFILE", &self.path)
            .env("NO_PROXY", "*")
            .env("KIRI_BASE_URL", base_url)
            .env("KIRI_API_KEY", "test-key")
            .env("KIRI_MODEL", "test-model")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        command
    }

    pub fn kiri_dir(&self) -> PathBuf {
        self.path.join(".kiri")
    }

    pub fn log_path(&self) -> PathBuf {
        self.kiri_dir().join("logs").join("daemon.jsonl")
    }

    pub fn log(&self) -> String {
        fs::read_to_string(self.log_path()).unwrap_or_default()
    }

    pub fn started_daemons(&self) -> usize {
        self.log().matches("daemon started").count()
    }

    pub fn wait_until(&self, condition: impl Fn() -> bool) {
        wait_until(condition);
    }

    pub fn wait_for_daemon_exit(&self) {
        let lock_path = self.kiri_dir().join("daemon.lock");
        self.wait_until(|| {
            File::options()
                .write(true)
                .open(&lock_path)
                .is_ok_and(|lock| lock.try_lock().is_ok())
        });
    }

    pub fn start_daemon_on(&self, base_url: &str) -> Child {
        let daemon = self.kiri_on(base_url, &["daemon"]).spawn().unwrap();
        self.wait_until(|| self.started_daemons() > 0);
        daemon
    }

    pub fn raw_client(&self) -> RawClient {
        let mut client = RawClient {
            stream: BufReader::new(self.open_endpoint().unwrap()),
        };
        let answer = client.call(0, "initialize", json!({"protocolVersion": 1}));
        assert_eq!(answer["result"]["protocolVersion"], 1);
        client
    }

    #[cfg(unix)]
    pub fn open_endpoint(&self) -> std::io::Result<Stream> {
        let stream = Stream::connect(self.kiri_dir().join("daemon.sock"))?;
        stream.set_read_timeout(Some(READ_TIMEOUT))?;
        Ok(stream)
    }

    #[cfg(windows)]
    pub fn open_endpoint(&self) -> std::io::Result<Stream> {
        let output = Command::new("whoami")
            .args(["/user", "/fo", "csv", "/nh"])
            .output()?;
        let line = String::from_utf8_lossy(&output.stdout);
        let sid = line.trim().rsplit(',').next().unwrap_or_default();
        File::options()
            .read(true)
            .write(true)
            .open(format!(r"\\.\pipe\kiri-{}", sid.trim_matches('"')))
    }
}

impl Drop for TestHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

pub fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "condition not met in 10 seconds");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(unix)]
type Stream = std::os::unix::net::UnixStream;
#[cfg(windows)]
type Stream = File;

pub struct RawClient {
    stream: BufReader<Stream>,
}

impl RawClient {
    pub fn write(&mut self, bytes: &[u8]) -> std::io::Result<()> {
        self.stream.get_mut().write_all(bytes)
    }

    pub fn send(&mut self, message: Value) {
        let mut line = serde_json::to_vec(&message).unwrap();
        line.push(b'\n');
        self.write(&line).unwrap();
    }

    pub fn request(&mut self, id: i64, method: &str, params: Value) {
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}));
    }

    pub fn read(&mut self) -> Option<Value> {
        let mut line = String::new();
        match self.stream.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(serde_json::from_str(&line).unwrap()),
        }
    }

    pub fn read_answer_to(&mut self, id: i64) -> Value {
        loop {
            let message = self
                .read()
                .expect("the connection closed before the answer");
            if message["id"] == id {
                return message;
            }
        }
    }

    pub fn call(&mut self, id: i64, method: &str, params: Value) -> Value {
        self.request(id, method, params);
        self.read_answer_to(id)
    }

    pub fn new_session(&mut self, id: i64) -> String {
        let answer = self.call(
            id,
            "session/new",
            json!({"cwd": absolute_path(), "mcpServers": []}),
        );
        answer["result"]["sessionId"].as_str().unwrap().to_owned()
    }
}

pub fn absolute_path() -> PathBuf {
    std::env::temp_dir()
}

pub struct Client {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: Receiver<u8>,
}

impl Client {
    pub fn start(mut command: Command) -> Self {
        let mut child = command.spawn().unwrap();
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().unwrap();
        let (bytes, received) = mpsc::channel();
        std::thread::spawn(move || {
            for byte in BufReader::new(stdout).bytes().map_while(Result::ok) {
                if bytes.send(byte).is_err() {
                    return;
                }
            }
        });
        Self {
            child,
            stdin,
            stdout: received,
        }
    }

    pub fn read_until(&mut self, expected: &str) -> String {
        self.read_until_within(expected, READ_TIMEOUT)
    }

    pub fn read_until_within(&mut self, expected: &str, limit: Duration) -> String {
        let mut seen = Vec::new();
        while !seen.ends_with(expected.as_bytes()) {
            match self.stdout.recv_timeout(limit) {
                Ok(byte) => seen.push(byte),
                Err(cause) => panic!(
                    "no {expected:?} on stdout ({cause}), got {:?}",
                    String::from_utf8_lossy(&seen)
                ),
            }
        }
        String::from_utf8(seen).unwrap()
    }

    pub fn send_line(&mut self, line: &str) {
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(line.as_bytes()).unwrap();
        stdin.write_all(b"\n").unwrap();
    }

    pub fn kill(mut self) {
        self.child.kill().unwrap();
        self.child.wait().unwrap();
    }

    pub fn end(mut self) {
        drop(self.stdin.take());
        assert_eq!(self.child.wait().unwrap().code(), Some(0));
    }

    pub fn finish(mut self) -> (Option<i32>, String) {
        drop(self.stdin.take());
        let output = self.child.wait_with_output().unwrap();
        (
            output.status.code(),
            String::from_utf8(output.stderr).unwrap(),
        )
    }
}
