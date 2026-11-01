use std::fs;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::process::{Child, ChildStdout};
use std::time::{Duration, Instant};

use crate::support::TestHome;

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

#[cfg(unix)]
#[test]
fn a_daemon_start_leaves_its_files_to_the_owner_alone() {
    use std::os::unix::fs::PermissionsExt;

    let home = TestHome::new();
    let mode_of = |path: PathBuf| fs::metadata(path).unwrap().permissions().mode() & 0o777;
    fs::create_dir_all(home.log_path().parent().unwrap()).unwrap();
    fs::write(home.log_path(), "").unwrap();
    fs::set_permissions(home.log_path(), fs::Permissions::from_mode(0o644)).unwrap();
    fs::set_permissions(home.kiri_dir(), fs::Permissions::from_mode(0o755)).unwrap();

    let mut client = home.kiri(&[]).spawn().unwrap();
    wait_for_input_prompt(&mut client);

    assert_eq!(mode_of(home.kiri_dir()), 0o700);
    assert_eq!(mode_of(home.kiri_dir().join("daemon.sock")), 0o600);
    assert_eq!(mode_of(home.kiri_dir().join("daemon.lock")), 0o600);
    assert_eq!(mode_of(home.log_path()), 0o600);

    end_client(client);
    home.wait_for_daemon_exit();
}

#[test]
fn a_kiri_path_the_daemon_cannot_secure_stops_it() {
    let home = TestHome::new();
    fs::write(home.kiri_dir(), "").unwrap();

    let output = home.kiri(&["daemon"]).output().unwrap();

    assert_eq!(output.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&output.stderr);
    let expected = format!("Could not secure {}: ", home.kiri_dir().display());
    assert!(stderr.contains(&expected), "{stderr}");
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
