#[cfg(unix)]
mod unix;
#[cfg(windows)]
mod windows;

use std::error::Error;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

#[cfg(unix)]
use unix as platform;
#[cfg(windows)]
use windows as platform;

type Outcome<T = ()> = Result<T, Box<dyn Error>>;

fn main() -> ExitCode {
    let arguments: Vec<String> = std::env::args().skip(1).collect();
    let arguments: Vec<&str> = arguments.iter().map(String::as_str).collect();
    match arguments.as_slice() {
        ["cross-user"] => match build_kiri().and_then(|kiri| platform::cross_user(&kiri)) {
            Ok(()) => {
                println!("Cross-user check passed.");
                ExitCode::SUCCESS
            }
            Err(cause) => {
                eprintln!("Cross-user check failed: {cause}");
                ExitCode::FAILURE
            }
        },
        #[cfg(unix)]
        ["probe", socket] => unix::probe(socket),
        _ => {
            eprintln!("Usage: cargo xtask cross-user");
            ExitCode::FAILURE
        }
    }
}

fn build_kiri() -> Outcome<PathBuf> {
    println!("Building kiri.");
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let status = Command::new(cargo)
        .args(["build", "--package", "kiri", "--locked"])
        .status()?;
    ensure(status.success(), "cargo build failed")?;
    Ok(Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../target/debug")
        .join(format!("kiri{}", std::env::consts::EXE_SUFFIX)))
}

fn ensure(condition: bool, failure: impl Into<String>) -> Outcome {
    if condition {
        Ok(())
    } else {
        Err(failure.into().into())
    }
}

fn wait_until(limit: Duration, what: &str, condition: impl Fn() -> bool) -> Outcome {
    let deadline = Instant::now() + limit;
    while !condition() {
        ensure(Instant::now() < deadline, format!("timed out: {what}"))?;
        std::thread::sleep(Duration::from_millis(10));
    }
    Ok(())
}

struct TemporaryHome(PathBuf);

impl TemporaryHome {
    fn create(path: PathBuf) -> Outcome<Self> {
        fs::create_dir_all(&path)?;
        Ok(Self(path))
    }

    fn kiri_dir(&self) -> PathBuf {
        self.0.join(".kiri")
    }

    fn log_path(&self) -> PathBuf {
        self.kiri_dir().join("logs").join("daemon.jsonl")
    }

    fn kiri(&self, kiri: &Path) -> Command {
        let mut command = Command::new(kiri);
        command
            .env("HOME", &self.0)
            .env("USERPROFILE", &self.0)
            .env("KIRI_BASE_URL", "http://127.0.0.1:1")
            .env("KIRI_API_KEY", "unused")
            .env("KIRI_MODEL", "unused");
        command
    }
}

impl Drop for TemporaryHome {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct OwnerClient {
    child: Child,
    lock: PathBuf,
}

impl OwnerClient {
    fn start(kiri: &Path, home: &TemporaryHome) -> Outcome<Self> {
        let mut child = home
            .kiri(kiri)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let lock = home.kiri_dir().join("daemon.lock");
        let mut prompt = [0; 2];
        let reached = child
            .stdout
            .take()
            .ok_or("the client has no stdout")?
            .read_exact(&mut prompt);
        let client = Self { child, lock };
        reached?;
        ensure(&prompt == b"> ", "the client printed no prompt")?;
        Ok(client)
    }
}

impl Drop for OwnerClient {
    fn drop(&mut self) {
        drop(self.child.stdin.take());
        let _ = self.child.wait();
        let lock = &self.lock;
        let _ = wait_until(Duration::from_secs(10), "the daemon to exit", || {
            File::options()
                .write(true)
                .open(lock)
                .is_ok_and(|lock| lock.try_lock().is_ok())
        });
    }
}
