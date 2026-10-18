use crate::Config;
use anyhow::{Context, Result};
use chrono::Utc;
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};
use tide_git::{failure_kind, FailureKind, GitBackend, LibgitBackend, SyncPhase, SyncReport};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Run {
    pub run_id: String,
    pub folder_id: String,
    pub started: String,
    pub elapsed_ms: u128,
    pub success: bool,
    pub report: Option<SyncReport>,
    pub error: Option<String>,
    #[serde(default)]
    pub failure_kind: Option<FailureKind>,
}

pub fn open_folder(config_path: &Path, name: &str) -> Result<LibgitBackend> {
    let config = Config::load(config_path)?;
    LibgitBackend::open(config.folder(name)?.options(&config.app))
}

pub fn sync_folder(config_path: &Path, name: &str, confirm_initial: bool) -> Result<Run> {
    sync_folder_observed(config_path, name, confirm_initial, |_| {})
}
pub fn sync_folder_observed(
    config_path: &Path,
    name: &str,
    confirm_initial: bool,
    observer: impl Fn(SyncPhase) + Send + 'static,
) -> Result<Run> {
    let config = Config::load(config_path)?;
    sync_with_config(config_path, &config, name, confirm_initial, observer)
}
pub(crate) fn sync_with_config(
    config_path: &Path,
    config: &Config,
    name: &str,
    confirm_initial: bool,
    observer: impl Fn(SyncPhase) + Send + 'static,
) -> Result<Run> {
    let folder = config.folder(name)?;
    anyhow::ensure!(!folder.paused, "folder is paused; use `tide resume` first");
    let now = Instant::now();
    let started = Utc::now().to_rfc3339();
    let run_id = format!("r-{}", uuid::Uuid::new_v4());
    let result = LibgitBackend::open(folder.options(&config.app)).and_then(|mut git| {
        git.set_observer(observer);
        git.sync(confirm_initial)
    });
    let mut run = Run {
        run_id,
        folder_id: folder.id.clone(),
        started,
        elapsed_ms: now.elapsed().as_millis(),
        success: false,
        report: None,
        error: None,
        failure_kind: None,
    };
    match result {
        Ok(report) => {
            run.success = !report.pending;
            run.report = Some(report);
        }
        Err(error) => {
            let mut message = format!("{error:#}");
            let kind = failure_kind(&error);
            run.failure_kind = Some(kind);
            if matches!(
                kind,
                FailureKind::DeletionGuard | FailureKind::MissingFolder
            ) {
                if let Err(pause_error) = Config::edit(config_path, |c| {
                    c.folders
                        .iter_mut()
                        .find(|f| f.id == folder.id)
                        .context("folder removed")?
                        .paused = true;
                    Ok(())
                }) {
                    message.push_str(&format!("; could not persist pause: {pause_error:#}"));
                }
            }
            run.error = Some(message);
        }
    }
    save_run(config_path, &run)?;
    Ok(run)
}

fn save_run(config: &Path, run: &Run) -> Result<()> {
    let parent = config.parent().context("config has no parent")?;
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(parent.join("runs.lock"))?;
    lock.lock_exclusive()?;
    let path = parent.join("runs.jsonl");
    let existing = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e.into()),
    };
    let lines: Vec<_> = existing.lines().collect();
    let mut temp = tempfile::NamedTempFile::new_in(parent)?;
    for line in lines.iter().skip(lines.len().saturating_sub(999)) {
        writeln!(temp, "{line}")?;
    }
    serde_json::to_writer(&mut temp, run)?;
    writeln!(temp)?;
    temp.as_file().sync_all()?;
    temp.persist(path).map_err(|e| e.error)?;
    Ok(())
}

pub fn runs(config: &Path) -> Result<Vec<Run>> {
    let path = config
        .parent()
        .context("config has no parent")?
        .join("runs.jsonl");
    if !path.exists() {
        return Ok(vec![]);
    }
    fs::read_to_string(path)?
        .lines()
        .rev()
        .map(|line| Ok(serde_json::from_str(line)?))
        .collect()
}

/// Foreground client of the shared service; configuration reloads automatically.
pub fn watch(config_path: PathBuf) -> Result<()> {
    let (service, events) = crate::Service::start(config_path.clone())?;
    println!(
        "Watching {} folder(s); Ctrl+C to stop. Configuration reloads automatically.",
        Config::load(&config_path)?.folders.len()
    );
    for event in events {
        match event {
            crate::ServiceEvent::Completed {
                payload: crate::Payload::Sync(run),
                ..
            } => println!("{}", serde_json::to_string(&run)?),
            crate::ServiceEvent::Failed { error, .. } | crate::ServiceEvent::Warning(error) => {
                eprintln!("{error}")
            }
            _ => (),
        }
    }
    service.stop();
    Ok(())
}
