use std::{
    fs,
    io::{BufRead, BufReader},
    path::Path,
    process::{Child, Command, Output, Stdio},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

fn tide(config: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_tide"))
        .arg("--config")
        .arg(config)
        .args(args)
        .output()
        .unwrap()
}
fn success(config: &Path, args: &[&str]) -> Output {
    let out = tide(config, args);
    assert!(
        out.status.success(),
        "{args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out
}
fn fixture() -> (
    tempfile::TempDir,
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
) {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let docs = temp.path().join("docs");
    let remote = temp.path().join("remote.git");
    fs::create_dir(&docs).unwrap();
    git2::Repository::init_bare(&remote).unwrap();
    success(&config, &["init"]);
    success(
        &config,
        &[
            "add",
            "notes",
            docs.to_str().unwrap(),
            remote.to_str().unwrap(),
        ],
    );
    (temp, config, docs, remote)
}

#[test]
fn command_line_lifecycle_and_error_exit_codes() {
    let (_temp, config, docs, _remote) = fixture();
    assert!(!tide(&config, &["init"]).status.success());
    fs::write(docs.join("note.md"), b"hello\n").unwrap();
    thread::sleep(Duration::from_millis(2100));
    let output = success(&config, &["sync", "notes"]);
    let run: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(run["success"], true);
    let history = success(&config, &["history", "notes"]);
    let entries: serde_json::Value = serde_json::from_slice(&history.stdout).unwrap();
    assert_eq!(entries.as_array().unwrap().len(), 1);
    success(&config, &["pause", "notes"]);
    assert!(!tide(&config, &["sync", "notes"]).status.success());
    success(&config, &["resume", "notes"]);
    success(&config, &["sync", "notes"]);
    success(&config, &["runs"]);
    success(&config, &["remove", "notes"]);
    assert!(docs.join("note.md").exists());
    assert!(docs.join(".git").exists());
    assert!(!tide(&config, &["sync", "unknown"]).status.success());
}

struct StopOnDrop(Child);
impl Drop for StopOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
fn real_filesystem_event_triggers_foreground_watcher() {
    let (_temp, config, docs, remote) = fixture();
    let text = fs::read_to_string(&config)
        .unwrap()
        .replace("interval = \"15m\"", "interval = \"0s\"")
        .replace("debounce = \"30s\"", "debounce = \"3s\"");
    fs::write(&config, text).unwrap();
    let mut child = StopOnDrop(
        Command::new(env!("CARGO_BIN_EXE_tide"))
            .arg("--config")
            .arg(&config)
            .arg("watch")
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap(),
    );
    let stdout = child.0.stdout.take().unwrap();
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            if tx.send(line.unwrap_or_default()).is_err() {
                break;
            }
        }
    });
    let ready = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert!(ready.starts_with("Watching"), "{ready}");
    fs::write(docs.join("watched.txt"), b"changed via OS event").unwrap();
    let deadline = Instant::now() + Duration::from_secs(15);
    let mut pushed = false;
    while Instant::now() < deadline {
        if git2::Repository::open_bare(&remote)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .is_ok()
        {
            pushed = true;
            break;
        }
        thread::sleep(Duration::from_millis(100));
    }
    assert!(pushed, "watcher did not push within 15s");
    drop(child);
    reader.join().unwrap();
}
