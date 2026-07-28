use std::ffi::{OsStr, OsString};
use std::io::Write;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};

const SUPPORTED_DISTROS: &[&str] = &[
    "Ubuntu-24.04",
    "Ubuntu-22.04",
    "Ubuntu-26.04",
    "Debian",
    "Debian-12",
    "Debian-13",
];
const LINUX_BWRAP: &str = "/usr/bin/bwrap";

#[derive(Debug, Clone, PartialEq, Eq)]
struct LauncherState {
    distro: String,
    payload: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LaunchSpec {
    program: PathBuf,
    args: Vec<OsString>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NetworkBridge {
    mode: String,
    windows_host: Option<Ipv4Addr>,
}

pub fn run() -> Result<()> {
    let args: Vec<OsString> = std::env::args_os().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "wsl") {
        return run_wsl_command(&args[1..]);
    }

    let wsl = wsl_executable()?;
    let state = load_or_discover_state(&wsl)?;
    verify_wsl2(&wsl, &state.distro)?;
    verify_supported_release(&wsl, &state.distro)?;
    verify_payload(&wsl, &state)?;
    verify_bwrap(&wsl, &state)?;
    let cwd = map_windows_path(&std::env::current_dir()?, &state.distro)?;
    let bridge = discover_network_bridge(&wsl, &state.distro)?;
    let spec = build_launch_spec(&wsl, &state, &cwd, &args, &bridge);
    let status = run_wsl(&spec.program, &spec.args)?;
    std::process::exit(status.code().unwrap_or(1));
}

fn run_wsl_command(args: &[OsString]) -> Result<()> {
    let action = args
        .first()
        .and_then(|value| value.to_str())
        .unwrap_or("status");
    let wsl = wsl_executable()?;
    match action {
        "status" => {
            let state = load_or_discover_state(&wsl)?;
            verify_wsl2(&wsl, &state.distro)?;
            verify_supported_release(&wsl, &state.distro)?;
            let payload = payload_exists(&wsl, &state)?;
            let bwrap = bwrap_version(&wsl, &state.distro)?;
            let bwrap_ready = bwrap.is_some() && bwrap_works(&wsl, &state)?;
            let bridge = discover_network_bridge(&wsl, &state.distro)?;
            println!("WSL2 distro: {}", state.distro);
            println!("WSL networking: {}", bridge.mode);
            println!(
                "Kiri payload: {}",
                if payload { "ready" } else { "missing" }
            );
            println!(
                "bubblewrap: {}",
                match (bwrap.as_deref(), bwrap_ready) {
                    (Some(version), true) => format!("{version} (ready)"),
                    (Some(version), false) => format!("{version} (unusable)"),
                    (None, _) => "missing".to_string(),
                }
            );
            Ok(())
        }
        "use" => {
            let distro = args
                .get(1)
                .and_then(|value| value.to_str())
                .ok_or_else(|| anyhow!("usage: kiri wsl use <distro>"))?;
            let state = discover_state_for(&wsl, distro)?;
            install_cached_payload(&wsl, &state)?;
            write_state(&state)?;
            println!("WSL2 distro selected: {}", state.distro);
            Ok(())
        }
        "repair" => {
            let current = load_state_for_repair(&wsl)?;
            let repaired = discover_state_for(&wsl, &current.distro)?;
            install_cached_payload(&wsl, &repaired)?;
            write_state(&repaired)?;
            println!("WSL runtime repaired for {}", repaired.distro);
            Ok(())
        }
        "clean" => {
            let state = load_or_discover_state(&wsl)?;
            clean_old_payloads(&wsl, &state)?;
            println!("Old WSL runtime versions removed from {}", state.distro);
            Ok(())
        }
        "remove" => {
            let state = load_or_discover_state(&wsl)?;
            remove_payload(&wsl, &state)?;
            remove_state_file(&state_path()?)?;
            println!("Kiri runtime removed from {}", state.distro);
            Ok(())
        }
        _ => bail!(
            "unknown WSL action {action:?}; use status, use <distro>, repair, clean, or remove"
        ),
    }
}

fn wsl_executable() -> Result<PathBuf> {
    let system_root = std::env::var_os("SystemRoot")
        .ok_or_else(|| anyhow!("SystemRoot is required to locate the trusted wsl.exe"))?;
    let path = PathBuf::from(system_root).join("System32").join("wsl.exe");
    if !path.is_file() {
        bail!(
            "WSL2 is required but {} was not found; install it with `wsl --install -d Ubuntu-24.04`",
            path.display()
        );
    }
    Ok(path)
}

fn state_path() -> Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| anyhow!("LOCALAPPDATA is required for Kiri launcher state"))?;
    Ok(PathBuf::from(local).join("Kiri").join("launcher.conf"))
}

fn cached_payload_path() -> Result<PathBuf> {
    let local = std::env::var_os("LOCALAPPDATA")
        .ok_or_else(|| anyhow!("LOCALAPPDATA is required for the cached Kiri payload"))?;
    Ok(PathBuf::from(local)
        .join("Kiri")
        .join("payloads")
        .join(env!("CARGO_PKG_VERSION"))
        .join("kiri"))
}

fn load_or_discover_state(wsl: &Path) -> Result<LauncherState> {
    let path = state_path()?;
    match read_state_file(&path)? {
        Some(raw) => parse_state(&raw),
        None => {
            let distro = discover_distro(wsl)?;
            let state = discover_state_for(wsl, &distro)?;
            write_state(&state)?;
            Ok(state)
        }
    }
}

fn load_state_for_repair(wsl: &Path) -> Result<LauncherState> {
    let path = state_path()?;
    match read_state_file(&path)? {
        Some(raw) => match parse_state(&raw) {
            Ok(state) => Ok(state),
            Err(error) => {
                eprintln!("warning: ignoring invalid launcher state during repair: {error}");
                let distro = discover_distro(wsl)?;
                discover_state_for(wsl, &distro)
            }
        },
        None => {
            let distro = discover_distro(wsl)?;
            discover_state_for(wsl, &distro)
        }
    }
}

fn parse_state(raw: &str) -> Result<LauncherState> {
    let mut lines = raw.lines();
    let distro = lines.next().unwrap_or_default().trim();
    let payload = lines.next().unwrap_or_default().trim();
    if lines.any(|line| !line.trim().is_empty()) {
        bail!("launcher state contains unexpected fields");
    }
    validate_distro(distro)?;
    let expected_suffix = format!("/.kiri/runtime/{}/kiri", env!("CARGO_PKG_VERSION"));
    if !payload.starts_with('/')
        || !payload.ends_with(&expected_suffix)
        || payload.contains(['\r', '\n', '\0'])
        || payload.split('/').any(|component| component == "..")
    {
        bail!("invalid Linux payload path in launcher state");
    }
    Ok(LauncherState {
        distro: distro.to_string(),
        payload: payload.to_string(),
    })
}

fn write_state(state: &LauncherState) -> Result<()> {
    let path = state_path()?;
    write_state_file(&path, state)
}

fn write_state_file(path: &Path, state: &LauncherState) -> Result<()> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("launcher state path has no parent"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let _lock = acquire_state_lock(path)?;
    recover_state_file(path)?;
    let contents = format!("{}\n{}\n", state.distro, state.payload);
    if std::fs::read_to_string(path).is_ok_and(|current| current == contents) {
        return Ok(());
    }
    let unique = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let temporary = path.with_extension(format!("{}.{}.tmp", std::process::id(), unique));
    let backup = path.with_extension("backup");
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temporary)
        .with_context(|| format!("failed to create {}", temporary.display()))?;
    file.write_all(contents.as_bytes())
        .with_context(|| format!("failed to write {}", temporary.display()))?;
    file.sync_all()
        .with_context(|| format!("failed to flush {}", temporary.display()))?;
    drop(file);
    if path.exists() {
        remove_if_exists(&backup)?;
        std::fs::rename(path, &backup)
            .with_context(|| format!("failed to preserve {}", path.display()))?;
    }
    if let Err(error) = std::fs::rename(&temporary, path) {
        if !path.exists() && backup.exists() {
            let _ = std::fs::rename(&backup, path);
        }
        let _ = std::fs::remove_file(&temporary);
        return Err(error).with_context(|| format!("failed to install {}", path.display()));
    }
    remove_if_exists(&backup)
}

fn read_state_file(path: &Path) -> Result<Option<String>> {
    let parent = path
        .parent()
        .ok_or_else(|| anyhow!("launcher state path has no parent"))?;
    std::fs::create_dir_all(parent)
        .with_context(|| format!("failed to create {}", parent.display()))?;
    let _lock = acquire_state_lock(path)?;
    recover_state_file(path)?;
    match std::fs::read_to_string(path) {
        Ok(contents) => Ok(Some(contents)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error).with_context(|| format!("failed to read {}", path.display())),
    }
}

fn remove_state_file(path: &Path) -> Result<()> {
    let _lock = acquire_state_lock(path)?;
    recover_state_file(path)?;
    remove_if_exists(path).with_context(|| format!("failed to remove {}", path.display()))
}

fn recover_state_file(path: &Path) -> Result<()> {
    let backup = path.with_extension("backup");
    if path.exists() {
        return remove_if_exists(&backup);
    }
    if backup.exists() {
        std::fs::rename(&backup, path)
            .with_context(|| format!("failed to recover {}", path.display()))?;
    }
    Ok(())
}

fn remove_if_exists(path: &Path) -> Result<()> {
    match std::fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("failed to remove {}", path.display())),
    }
}

struct StateLock {
    file: std::fs::File,
}

impl Drop for StateLock {
    fn drop(&mut self) {
        let _ = self.file.unlock();
    }
}

fn acquire_state_lock(path: &Path) -> Result<StateLock> {
    acquire_state_lock_with_timeout(path, Duration::from_secs(6))
}

fn acquire_state_lock_with_timeout(path: &Path, wait_timeout: Duration) -> Result<StateLock> {
    let lock = path.with_extension("lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&lock)
        .with_context(|| format!("failed to open launcher state lock {}", lock.display()))?;
    let deadline = SystemTime::now() + wait_timeout;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(StateLock { file }),
            Err(std::fs::TryLockError::WouldBlock) => {
                if SystemTime::now() >= deadline {
                    bail!(
                        "timed out waiting for launcher state lock {}",
                        lock.display()
                    );
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            Err(std::fs::TryLockError::Error(error)) => {
                return Err(error)
                    .with_context(|| format!("failed to lock launcher state {}", path.display()));
            }
        }
    }
}

fn discover_distro(wsl: &Path) -> Result<String> {
    let output = run_wsl_output(wsl, [OsStr::new("--list"), OsStr::new("--quiet")])?;
    let installed: Vec<String> = decode_wsl_output(&output.stdout)
        .lines()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !name.to_ascii_lowercase().contains("docker"))
        .map(str::to_string)
        .collect();
    SUPPORTED_DISTROS
        .iter()
        .find(|supported| installed.iter().any(|name| name == **supported))
        .map(|name| (*name).to_string())
        .ok_or_else(|| {
            anyhow!(
                "no supported WSL2 distro found; install Ubuntu 24.04 with `wsl --install -d Ubuntu-24.04`"
            )
        })
}

fn discover_state_for(wsl: &Path, distro: &str) -> Result<LauncherState> {
    validate_distro(distro)?;
    verify_wsl2(wsl, distro)?;
    verify_supported_release(wsl, distro)?;
    let output = run_wsl_output(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(distro),
            OsStr::new("--exec"),
            OsStr::new("/usr/bin/printenv"),
            OsStr::new("HOME"),
        ],
    )?;
    if !output.status.success() {
        bail!("could not resolve HOME inside WSL distro {distro}");
    }
    let home = decode_wsl_output(&output.stdout).trim().to_string();
    if !home.starts_with('/') || home.contains(['\r', '\n', '\0']) {
        bail!("WSL distro {distro} returned an invalid HOME path");
    }
    Ok(LauncherState {
        distro: distro.to_string(),
        payload: format!("{home}/.kiri/runtime/{}/kiri", env!("CARGO_PKG_VERSION")),
    })
}

fn validate_distro(distro: &str) -> Result<()> {
    if distro.is_empty()
        || distro.len() > 128
        || distro
            .chars()
            .any(|character| character.is_control() || matches!(character, '/' | '\\'))
    {
        bail!("invalid WSL distro name");
    }
    if !SUPPORTED_DISTROS.contains(&distro) {
        bail!("unsupported WSL distro {distro:?}");
    }
    Ok(())
}

fn verify_wsl2(wsl: &Path, distro: &str) -> Result<()> {
    validate_distro(distro)?;
    let output = run_wsl_output(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(distro),
            OsStr::new("--exec"),
            OsStr::new("/usr/bin/uname"),
            OsStr::new("-r"),
        ],
    )?;
    let kernel = decode_wsl_output(&output.stdout).to_ascii_lowercase();
    if !output.status.success() || !kernel.contains("wsl2") {
        bail!(
            "{distro} is unavailable or is not WSL2; convert it with `wsl --set-version {distro} 2`"
        );
    }
    Ok(())
}

fn verify_supported_release(wsl: &Path, distro: &str) -> Result<()> {
    let output = run_wsl_output(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(distro),
            OsStr::new("--exec"),
            OsStr::new("/usr/bin/cat"),
            OsStr::new("/etc/os-release"),
        ],
    )?;
    if !output.status.success() {
        bail!("could not identify the Linux release in {distro}");
    }
    let (identifier, version) = parse_os_release(&decode_wsl_output(&output.stdout))?;
    let supported = match distro {
        "Ubuntu-22.04" => identifier == "ubuntu" && version == "22.04",
        "Ubuntu-24.04" => identifier == "ubuntu" && version == "24.04",
        "Ubuntu-26.04" => identifier == "ubuntu" && version == "26.04",
        "Debian" => identifier == "debian" && matches!(version.as_str(), "12" | "13"),
        "Debian-12" => identifier == "debian" && version == "12",
        "Debian-13" => identifier == "debian" && version == "13",
        _ => false,
    };
    if !supported {
        bail!(
            "unsupported Linux release in {distro}: {identifier} {version}; use Ubuntu 22.04/24.04/26.04 or Debian 12/13"
        );
    }
    Ok(())
}

fn parse_os_release(raw: &str) -> Result<(String, String)> {
    fn value(raw: &str, key: &str) -> Option<String> {
        raw.lines().find_map(|line| {
            let (candidate, value) = line.split_once('=')?;
            (candidate == key).then(|| value.trim().trim_matches(['\'', '"']).to_ascii_lowercase())
        })
    }

    let identifier = value(raw, "ID").ok_or_else(|| anyhow!("/etc/os-release has no ID"))?;
    let version =
        value(raw, "VERSION_ID").ok_or_else(|| anyhow!("/etc/os-release has no VERSION_ID"))?;
    if identifier.is_empty()
        || version.is_empty()
        || identifier.chars().any(char::is_control)
        || version.chars().any(char::is_control)
    {
        bail!("/etc/os-release contains invalid values");
    }
    Ok((identifier, version))
}

fn discover_network_bridge(wsl: &Path, distro: &str) -> Result<NetworkBridge> {
    let output = run_wsl_output(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(distro),
            OsStr::new("--exec"),
            OsStr::new("/usr/bin/wslinfo"),
            OsStr::new("--networking-mode"),
        ],
    )?;
    let mode = if output.status.success() {
        decode_wsl_output(&output.stdout)
            .trim()
            .to_ascii_lowercase()
    } else {
        // Older WSL builds predate mirrored networking and `wslinfo`; their effective default is NAT.
        eprintln!("warning: wslinfo unavailable; assuming the legacy NAT networking mode");
        "nat".to_string()
    };
    if mode.is_empty()
        || mode
            .chars()
            .any(|character| !character.is_ascii_alphanumeric())
    {
        bail!("WSL returned an invalid networking mode");
    }
    let windows_host = if mode == "nat" {
        let route = run_wsl_output(
            wsl,
            [
                OsStr::new("--distribution"),
                OsStr::new(distro),
                OsStr::new("--exec"),
                OsStr::new("/usr/sbin/ip"),
                OsStr::new("-4"),
                OsStr::new("route"),
                OsStr::new("show"),
                OsStr::new("default"),
            ],
        )?;
        if !route.status.success() {
            bail!("could not resolve the Windows host address from the WSL default route");
        }
        Some(parse_default_gateway(&decode_wsl_output(&route.stdout))?)
    } else {
        None
    };
    Ok(NetworkBridge { mode, windows_host })
}

fn parse_default_gateway(route: &str) -> Result<Ipv4Addr> {
    let tokens: Vec<&str> = route.split_whitespace().collect();
    let gateway = tokens
        .windows(2)
        .find_map(|pair| (pair[0] == "via").then_some(pair[1]))
        .ok_or_else(|| anyhow!("WSL default route has no gateway"))?;
    gateway
        .parse()
        .map_err(|_| anyhow!("WSL default route contains an invalid IPv4 gateway"))
}

fn verify_payload(wsl: &Path, state: &LauncherState) -> Result<()> {
    if !payload_exists(wsl, state)? {
        bail!(
            "Kiri Linux payload {} is missing in {}; reinstall or run `kiri wsl repair`",
            state.payload,
            state.distro
        );
    }
    Ok(())
}

fn payload_exists(wsl: &Path, state: &LauncherState) -> Result<bool> {
    let status = run_wsl_status(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(&state.distro),
            OsStr::new("--exec"),
            OsStr::new("/usr/bin/test"),
            OsStr::new("-x"),
            OsStr::new(&state.payload),
        ],
    )?;
    Ok(status.success())
}

fn bwrap_version(wsl: &Path, distro: &str) -> Result<Option<String>> {
    let output = run_wsl_output(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(distro),
            OsStr::new("--exec"),
            OsStr::new(LINUX_BWRAP),
            OsStr::new("--version"),
        ],
    )?;
    if !output.status.success() {
        return Ok(None);
    }
    let version = decode_wsl_output(&output.stdout).trim().to_string();
    Ok((!version.is_empty()).then_some(version))
}

fn verify_bwrap(wsl: &Path, state: &LauncherState) -> Result<()> {
    if bwrap_version(wsl, &state.distro)?.is_none() || !bwrap_works(wsl, state)? {
        bail!(
            "bubblewrap is missing or unusable in {}; reinstall or run `kiri wsl repair`",
            state.distro
        );
    }
    Ok(())
}

fn bwrap_works(wsl: &Path, state: &LauncherState) -> Result<bool> {
    let args = bwrap_probe_args(state)?;
    Ok(run_wsl_status(wsl, args.iter().map(OsString::as_os_str))?.success())
}

fn bwrap_probe_args(state: &LauncherState) -> Result<Vec<OsString>> {
    let home = linux_home(state)?;
    let runtime = runtime_root(state)?;
    let private = format!("{home}/.kiri");
    let mut args: Vec<OsString> = ["--distribution", &state.distro, "--exec", LINUX_BWRAP]
        .into_iter()
        .map(OsString::from)
        .collect();
    for root in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"] {
        args.extend(["--ro-bind", root, root].into_iter().map(OsString::from));
    }
    args.extend(
        [
            "--dev",
            "/dev",
            "--proc",
            "/proc",
            "--tmpfs",
            "/tmp",
            "--bind",
            "/tmp",
            "/tmp",
            "--bind",
            home,
            home,
            "--tmpfs",
            &private,
            "--bind",
            runtime,
            runtime,
            "--unshare-net",
            "--unshare-pid",
            "--unshare-ipc",
            "--new-session",
            "--cap-drop",
            "ALL",
            "--die-with-parent",
            "--chdir",
            "/tmp",
            "--",
            "/usr/bin/true",
        ]
        .into_iter()
        .map(OsString::from),
    );
    Ok(args)
}

fn install_cached_payload(wsl: &Path, state: &LauncherState) -> Result<()> {
    let cached = cached_payload_path()?;
    if !cached.is_file() {
        bail!(
            "cached Linux payload {} is missing; reinstall Kiri for Windows",
            cached.display()
        );
    }
    let source = map_windows_path(&cached, &state.distro)?;
    let status = run_wsl_status(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(&state.distro),
            OsStr::new("--exec"),
            OsStr::new("/usr/bin/install"),
            OsStr::new("-D"),
            OsStr::new("-m"),
            OsStr::new("0755"),
            OsStr::new(&source),
            OsStr::new(&state.payload),
        ],
    )?;
    if !status.success() {
        bail!("failed to install the Linux payload in {}", state.distro);
    }
    verify_bwrap(wsl, state)?;
    verify_payload_runs(wsl, state)?;
    Ok(())
}

fn verify_payload_runs(wsl: &Path, state: &LauncherState) -> Result<()> {
    let output = run_wsl_output(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(&state.distro),
            OsStr::new("--exec"),
            OsStr::new(&state.payload),
            OsStr::new("--help"),
        ],
    )?;
    if !output.status.success() {
        bail!("the Linux payload cannot start in {}", state.distro);
    }
    Ok(())
}

fn runtime_root(state: &LauncherState) -> Result<&str> {
    state
        .payload
        .strip_suffix(&format!("/{}/kiri", env!("CARGO_PKG_VERSION")))
        .ok_or_else(|| anyhow!("invalid versioned Linux payload path"))
}

fn linux_home(state: &LauncherState) -> Result<&str> {
    runtime_root(state)?
        .strip_suffix("/.kiri/runtime")
        .filter(|home| !home.is_empty())
        .ok_or_else(|| anyhow!("invalid Linux home in payload path"))
}

fn clean_old_payloads(wsl: &Path, state: &LauncherState) -> Result<()> {
    let root = runtime_root(state)?;
    let status = run_wsl_status(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(&state.distro),
            OsStr::new("--exec"),
            OsStr::new("/usr/bin/find"),
            OsStr::new(root),
            OsStr::new("-mindepth"),
            OsStr::new("1"),
            OsStr::new("-maxdepth"),
            OsStr::new("1"),
            OsStr::new("-type"),
            OsStr::new("d"),
            OsStr::new("!"),
            OsStr::new("-name"),
            OsStr::new(env!("CARGO_PKG_VERSION")),
            OsStr::new("-exec"),
            OsStr::new("/usr/bin/rm"),
            OsStr::new("-rf"),
            OsStr::new("{}"),
            OsStr::new("+"),
        ],
    )?;
    if !status.success() {
        bail!("failed to clean old Kiri runtimes in {}", state.distro);
    }
    Ok(())
}

fn remove_payload(wsl: &Path, state: &LauncherState) -> Result<()> {
    let status = run_wsl_status(
        wsl,
        [
            OsStr::new("--distribution"),
            OsStr::new(&state.distro),
            OsStr::new("--exec"),
            OsStr::new("/usr/bin/rm"),
            OsStr::new("-f"),
            OsStr::new(&state.payload),
        ],
    )?;
    if !status.success() {
        bail!("failed to remove the Kiri runtime from {}", state.distro);
    }
    Ok(())
}

fn map_windows_path(path: &Path, distro: &str) -> Result<String> {
    let raw = path.to_string_lossy();
    let normalized = raw.strip_prefix(r"\\?\").unwrap_or(&raw);
    if normalized.starts_with(r"\\") {
        let components: Vec<&str> = normalized
            .split('\\')
            .filter(|part| !part.is_empty())
            .collect();
        if components.len() < 2
            || !components[0].eq_ignore_ascii_case("wsl.localhost")
            || !components[1].eq_ignore_ascii_case(distro)
        {
            bail!("only paths inside \\\\wsl.localhost\\{distro} are supported");
        }
        return Ok(format!("/{}", components[2..].join("/")));
    }
    let bytes = normalized.as_bytes();
    if bytes.len() < 3 || !bytes[0].is_ascii_alphabetic() || bytes[1] != b':' {
        bail!("the Windows working directory must be an absolute drive or selected WSL path");
    }
    let drive = (bytes[0] as char).to_ascii_lowercase();
    let tail = normalized[2..].replace('\\', "/");
    Ok(format!("/mnt/{drive}{tail}"))
}

fn build_launch_spec(
    wsl: &Path,
    state: &LauncherState,
    cwd: &str,
    user_args: &[OsString],
    bridge: &NetworkBridge,
) -> LaunchSpec {
    let mut args = vec![
        OsString::from("--distribution"),
        OsString::from(&state.distro),
        OsString::from("--cd"),
        OsString::from(cwd),
        OsString::from("--exec"),
        OsString::from("/usr/bin/env"),
        OsString::from(format!("KIRI_WSL_NETWORKING_MODE={}", bridge.mode)),
    ];
    if let Some(host) = bridge.windows_host {
        args.push(OsString::from(format!("KIRI_WINDOWS_HOST={host}")));
    }
    args.push(OsString::from(&state.payload));
    args.extend_from_slice(user_args);
    LaunchSpec {
        program: wsl.to_path_buf(),
        args,
    }
}

fn run_wsl(program: &Path, args: &[OsString]) -> Result<ExitStatus> {
    let mut command = trusted_wsl_command(program);
    command.args(args);
    command
        .status()
        .with_context(|| format!("failed to start trusted {}", program.display()))
}

fn run_wsl_status<'a>(
    program: &Path,
    args: impl IntoIterator<Item = &'a OsStr>,
) -> Result<ExitStatus> {
    let mut command = trusted_wsl_command(program);
    command.args(args);
    command
        .status()
        .with_context(|| format!("failed to start trusted {}", program.display()))
}

fn run_wsl_output<'a>(
    program: &Path,
    args: impl IntoIterator<Item = &'a OsStr>,
) -> Result<std::process::Output> {
    let mut command = trusted_wsl_command(program);
    command.args(args);
    command
        .output()
        .with_context(|| format!("failed to start trusted {}", program.display()))
}

fn trusted_wsl_command(program: &Path) -> Command {
    let system_root = std::env::var_os("SystemRoot");
    let windir = std::env::var_os("WINDIR");
    let mut command = Command::new(program);
    command.env_clear();
    if let Some(value) = system_root {
        command.env("SystemRoot", value);
    }
    if let Some(value) = windir {
        command.env("WINDIR", value);
    }
    command
}

fn decode_wsl_output(bytes: &[u8]) -> String {
    let utf16 = bytes.starts_with(&[0xff, 0xfe])
        || bytes
            .chunks_exact(2)
            .take(8)
            .filter(|pair| pair[1] == 0)
            .count()
            >= 3;
    if !utf16 {
        return String::from_utf8_lossy(bytes).into_owned();
    }
    let words: Vec<u16> = bytes
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
        .filter(|word| *word != 0xfeff)
        .collect();
    String::from_utf16_lossy(&words)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_windows_drive_cwd_without_losing_unicode_or_spaces() {
        assert_eq!(
            map_windows_path(Path::new(r"C:\Users\Theo\Meu Projeto\ação"), "Ubuntu-24.04").unwrap(),
            "/mnt/c/Users/Theo/Meu Projeto/ação"
        );
    }

    #[test]
    fn accepts_only_the_selected_wsl_unc_namespace() {
        assert_eq!(
            map_windows_path(
                Path::new(r"\\wsl.localhost\Ubuntu-24.04\home\theo\repo"),
                "Ubuntu-24.04"
            )
            .unwrap(),
            "/home/theo/repo"
        );
        assert!(
            map_windows_path(
                Path::new(r"\\wsl.localhost\Debian\home\theo"),
                "Ubuntu-24.04"
            )
            .is_err()
        );
        assert!(map_windows_path(Path::new(r"\\server\share\repo"), "Ubuntu-24.04").is_err());
    }

    #[test]
    fn launch_spec_uses_absolute_wsl_and_versioned_payload() {
        let state = LauncherState {
            distro: "Ubuntu-24.04".to_string(),
            payload: "/home/theo/.kiri/runtime/0.1.0/kiri".to_string(),
        };
        let spec = build_launch_spec(
            Path::new(r"C:\Windows\System32\wsl.exe"),
            &state,
            "/mnt/c/work space",
            &[OsString::from("--version")],
            &NetworkBridge {
                mode: "nat".to_string(),
                windows_host: Some(Ipv4Addr::new(172, 30, 96, 1)),
            },
        );
        assert_eq!(spec.program, PathBuf::from(r"C:\Windows\System32\wsl.exe"));
        assert_eq!(
            spec.args,
            [
                "--distribution",
                "Ubuntu-24.04",
                "--cd",
                "/mnt/c/work space",
                "--exec",
                "/usr/bin/env",
                "KIRI_WSL_NETWORKING_MODE=nat",
                "KIRI_WINDOWS_HOST=172.30.96.1",
                "/home/theo/.kiri/runtime/0.1.0/kiri",
                "--version",
            ]
        );
    }

    #[test]
    fn decodes_utf16_output_from_wsl_list() {
        let words: Vec<u16> = "Ubuntu-24.04\r\n".encode_utf16().collect();
        let bytes: Vec<u8> = words.into_iter().flat_map(u16::to_le_bytes).collect();
        assert_eq!(decode_wsl_output(&bytes), "Ubuntu-24.04\r\n");
    }

    #[test]
    fn state_rejects_injected_distro_and_relative_payload() {
        assert!(parse_state("Ubuntu\n--exec\n").is_err());
        assert!(parse_state("Ubuntu-24.04\nrelative/kiri\n").is_err());
        assert!(parse_state("Ubuntu-24.04\n/home/theo/../.kiri/runtime/0.1.0/kiri\n").is_err());
        assert!(parse_state("Ubuntu-24.04\n/home/theo/anything/kiri\n").is_err());
        assert!(parse_state("Ubuntu-24.04\n/home/theo/.kiri/runtime/0.1.0/kiri\nextra\n").is_err());
        assert!(parse_state("Arch\n/home/theo/.kiri/runtime/0.1.0/kiri\n").is_err());
    }

    #[test]
    fn runtime_root_is_derived_only_from_the_versioned_payload() {
        let state = LauncherState {
            distro: "Ubuntu-24.04".to_string(),
            payload: "/home/theo/.kiri/runtime/0.1.0/kiri".to_string(),
        };
        assert_eq!(runtime_root(&state).unwrap(), "/home/theo/.kiri/runtime");
    }

    #[test]
    fn parses_only_the_gateway_after_via() {
        assert_eq!(
            parse_default_gateway("default via 172.30.96.1 dev eth0 proto kernel\n").unwrap(),
            Ipv4Addr::new(172, 30, 96, 1)
        );
        assert!(parse_default_gateway("default dev eth0\n").is_err());
        assert!(parse_default_gateway("default via attacker.example dev eth0\n").is_err());
    }

    #[test]
    fn parses_only_supported_os_release_fields() {
        assert_eq!(
            parse_os_release("NAME=Ubuntu\nID=ubuntu\nVERSION_ID=\"24.04\"\n").unwrap(),
            ("ubuntu".to_string(), "24.04".to_string())
        );
        assert!(parse_os_release("ID=ubuntu\n").is_err());
        assert!(parse_os_release("VERSION_ID=24.04\n").is_err());
    }

    #[test]
    fn launcher_state_rewrite_is_idempotent() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("launcher.conf");
        let state = LauncherState {
            distro: "Ubuntu-24.04".to_string(),
            payload: "/home/theo/.kiri/runtime/0.1.0/kiri".to_string(),
        };

        write_state_file(&path, &state).unwrap();
        let first = std::fs::read(&path).unwrap();
        write_state_file(&path, &state).unwrap();

        assert_eq!(std::fs::read(path).unwrap(), first);
    }

    #[test]
    fn launcher_state_replaces_content_without_losing_a_valid_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("launcher.conf");
        let first = LauncherState {
            distro: "Ubuntu-24.04".to_string(),
            payload: "/home/theo/.kiri/runtime/0.1.0/kiri".to_string(),
        };
        let second = LauncherState {
            distro: "Debian-13".to_string(),
            payload: "/home/theo/.kiri/runtime/0.1.0/kiri".to_string(),
        };

        write_state_file(&path, &first).unwrap();
        write_state_file(&path, &second).unwrap();

        assert_eq!(
            parse_state(&std::fs::read_to_string(&path).unwrap()).unwrap(),
            second
        );
        assert!(!path.with_extension("backup").exists());
    }

    #[test]
    fn launcher_state_recovers_an_interrupted_replacement() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("launcher.conf");
        let backup = path.with_extension("backup");
        std::fs::write(
            &backup,
            "Ubuntu-24.04\n/home/theo/.kiri/runtime/0.1.0/kiri\n",
        )
        .unwrap();

        let recovered = read_state_file(&path).unwrap().unwrap();

        assert_eq!(parse_state(&recovered).unwrap().distro, "Ubuntu-24.04");
        assert!(path.exists());
        assert!(!backup.exists());
    }

    #[test]
    fn launcher_state_serializes_concurrent_writers() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("launcher.conf");
        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));
        let states = [
            LauncherState {
                distro: "Ubuntu-24.04".to_string(),
                payload: "/home/theo/.kiri/runtime/0.1.0/kiri".to_string(),
            },
            LauncherState {
                distro: "Debian-13".to_string(),
                payload: "/home/theo/.kiri/runtime/0.1.0/kiri".to_string(),
            },
        ];
        let handles: Vec<_> = states
            .iter()
            .cloned()
            .map(|state| {
                let path = path.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    write_state_file(&path, &state).unwrap();
                })
            })
            .collect();
        for handle in handles {
            handle.join().unwrap();
        }

        let stored = parse_state(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(states.contains(&stored));
        let leftovers: Vec<_> = std::fs::read_dir(directory.path())
            .unwrap()
            .filter_map(Result::ok)
            .filter(|entry| entry.path() != path && entry.path() != path.with_extension("lock"))
            .collect();
        assert!(
            leftovers.is_empty(),
            "leftover transaction files: {leftovers:?}"
        );
    }

    #[test]
    fn launcher_state_never_steals_an_active_os_lock() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("launcher.conf");
        let holder = acquire_state_lock(&path).unwrap();

        let blocked = acquire_state_lock_with_timeout(&path, Duration::from_millis(30))
            .err()
            .expect("an active owner keeps the lock");
        assert!(blocked.to_string().contains("timed out"));

        drop(holder);
        let recovered = acquire_state_lock_with_timeout(&path, Duration::from_millis(100)).unwrap();
        drop(recovered);
    }

    #[test]
    fn bwrap_probe_uses_only_absolute_programs_and_required_isolation() {
        let state = LauncherState {
            distro: "Ubuntu-24.04".to_string(),
            payload: "/home/theo/.kiri/runtime/0.1.0/kiri".to_string(),
        };
        let args = bwrap_probe_args(&state).unwrap();
        let args: Vec<String> = args
            .iter()
            .map(|argument| argument.to_string_lossy().into_owned())
            .collect();

        assert!(args.contains(&LINUX_BWRAP.to_string()));
        assert!(args.contains(&"/usr/bin/true".to_string()));
        for root in ["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc"] {
            assert!(
                args.contains(&root.to_string()),
                "missing system root {root}"
            );
        }
        for path in ["/tmp", "/home/theo", "/home/theo/.kiri/runtime"] {
            assert!(
                args.contains(&path.to_string()),
                "missing representative bind {path}"
            );
        }
        assert!(
            args.windows(2)
                .any(|pair| pair == ["--tmpfs", "/home/theo/.kiri"])
        );
        for flag in [
            "--unshare-net",
            "--unshare-pid",
            "--unshare-ipc",
            "--new-session",
            "--die-with-parent",
        ] {
            assert!(args.contains(&flag.to_string()), "missing {flag}");
        }
    }
}
