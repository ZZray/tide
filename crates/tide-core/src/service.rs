//! Event-driven coordinator shared by desktop and CLI. Git runs on worker threads.
use crate::{duration, runs, scheduler::Schedule, Config, Run};
use anyhow::{Context, Result};
use fs2::FileExt;
use notify::{RecursiveMode, Watcher};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::OpenOptions,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, Sender},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};
use tide_git::{
    AssociationPlan, ChangeEntry, Conflict, FailureKind, GitBackend, HistoryEntry, SyncPhase,
};

#[derive(Clone, Debug)]
pub enum Task {
    Sync {
        confirm_initial: bool,
    },
    Changes,
    Plan,
    History {
        skip: usize,
    },
    Conflicts,
    Runs,
    Diff {
        revision: String,
        path: Option<String>,
    },
    Restore {
        revision: String,
        path: Option<String>,
    },
    Overwrite {
        use_local: bool,
        confirm_initial: bool,
    },
    Resolve {
        path: String,
        local: bool,
    },
    Export {
        revision: String,
        path: String,
        destination: PathBuf,
    },
    Recover,
}
impl Task {
    fn is_sync(&self) -> bool {
        matches!(self, Self::Sync { .. })
    }
    fn changes_documents(&self) -> bool {
        matches!(
            self,
            Self::Restore { .. } | Self::Overwrite { .. } | Self::Resolve { .. } | Self::Recover
        )
    }
}
#[derive(Clone, Debug)]
pub enum Payload {
    Sync(Run),
    Changes(Vec<ChangeEntry>),
    Plan(AssociationPlan),
    History(Vec<HistoryEntry>),
    Conflicts(Vec<Conflict>),
    Runs(Vec<Run>),
    Diff(String),
    Done,
}
#[derive(Clone, Debug)]
pub enum ServiceEvent {
    Config(Config),
    Changed {
        id: String,
    },
    Started {
        id: String,
        request: u64,
        task: Task,
    },
    Phase {
        id: String,
        phase: SyncPhase,
    },
    Completed {
        id: String,
        request: u64,
        payload: Payload,
    },
    Failed {
        id: String,
        request: u64,
        error: String,
        kind: FailureKind,
    },
    Warning(String),
}
enum Message {
    Run {
        id: String,
        request: u64,
        task: Task,
    },
    Reload,
    Wake,
    Stop,
    Paths(Vec<PathBuf>),
    WatchError(String),
    Done {
        id: String,
        request: u64,
        task: Task,
        result: Box<std::result::Result<Payload, (FailureKind, String)>>,
    },
}
#[derive(Clone)]
pub struct Service {
    tx: Sender<Message>,
    worker: Arc<Mutex<Option<thread::JoinHandle<()>>>>,
}
impl Service {
    pub fn start(path: PathBuf) -> Result<(Self, Receiver<ServiceEvent>)> {
        let config = Config::load(&path)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("watch.lock"))?;
        lock.try_lock_exclusive()
            .context("a watcher/desktop is already running for this configuration")?;
        let (tx, rx) = mpsc::channel();
        let (events, receiver) = mpsc::channel();
        let service_tx = tx.clone();
        let (ready, initialized) = mpsc::channel();
        let worker = thread::spawn(move || {
            let _lock = lock;
            if let Err(error) = coordinate(path, config, tx, rx, events.clone(), ready.clone()) {
                let message = format!("background service stopped: {error:#}");
                let _ = ready.send(Err(message.clone()));
                let _ = events.send(ServiceEvent::Warning(message));
            }
        });
        let service = Self {
            tx: service_tx,
            worker: Arc::new(Mutex::new(Some(worker))),
        };
        initialized
            .recv_timeout(Duration::from_secs(10))
            .context("background service initialization timed out")?
            .map_err(anyhow::Error::msg)?;
        Ok((service, receiver))
    }
    pub fn run(&self, id: impl Into<String>, request: u64, task: Task) {
        let _ = self.tx.send(Message::Run {
            id: id.into(),
            request,
            task,
        });
    }
    pub fn reload(&self) {
        let _ = self.tx.send(Message::Reload);
    }
    pub fn wake(&self) {
        let _ = self.tx.send(Message::Wake);
    }
    pub fn stop(&self) {
        let _ = self.tx.send(Message::Stop);
    }
    /// Finish active work and release the configuration lock before process exit.
    pub fn shutdown(&self) -> Result<()> {
        self.stop();
        if let Some(worker) = self
            .worker
            .lock()
            .map_err(|_| anyhow::anyhow!("service join lock poisoned"))?
            .take()
        {
            worker
                .join()
                .map_err(|_| anyhow::anyhow!("service coordinator panicked"))?;
        }
        Ok(())
    }
}
fn add_watches(
    watcher: &mut impl Watcher,
    config: &Config,
    watched: &mut HashSet<PathBuf>,
    events: &Sender<ServiceEvent>,
) {
    let desired: HashSet<_> = config
        .folders
        .iter()
        .filter(|f| f.on_change && !f.paused && f.path.is_dir())
        .map(|f| f.path.clone())
        .collect();
    for p in watched.difference(&desired) {
        let _ = watcher.unwatch(p);
    }
    for p in desired.difference(watched) {
        if let Err(e) = watcher.watch(p, RecursiveMode::Recursive) {
            let _ = events.send(ServiceEvent::Warning(format!(
                "cannot watch {}: {e}",
                p.display()
            )));
        }
    }
    *watched = desired;
}
fn update_schedules(
    config: &Config,
    schedules: &mut HashMap<String, Schedule>,
    now: u64,
    startup: bool,
) -> Result<()> {
    schedules.retain(|id, _| config.folders.iter().any(|f| f.id == *id));
    for folder in &config.folders {
        let is_new = !schedules.contains_key(&folder.id);
        let s = schedules
            .entry(folder.id.clone())
            .or_insert_with(|| Schedule::new(0, 30, now, folder.paused));
        let was_disabled = s.disabled;
        let previous_interval = s.interval;
        s.interval = duration(&folder.interval)?.as_secs();
        s.debounce = duration(&folder.debounce)?.as_secs();
        if s.user_paused && !folder.paused {
            s.retry_stopped = false;
        }
        s.user_paused = folder.paused;
        s.disabled = folder.paused || s.retry_stopped;
        if !s.running && previous_interval != s.interval {
            s.due = if s.interval == 0 {
                None
            } else {
                Some(now + s.interval)
            };
        }
        if !s.disabled && (startup || is_new || was_disabled) && s.interval > 0 {
            s.request(now + 10);
        }
    }
    Ok(())
}
fn coordinate(
    path: PathBuf,
    mut config: Config,
    tx: Sender<Message>,
    rx: Receiver<Message>,
    events: Sender<ServiceEvent>,
    ready: Sender<std::result::Result<(), String>>,
) -> Result<()> {
    let watcher_tx = tx.clone();
    let mut watcher = notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
        let message = match event {
            Ok(e) if !matches!(e.kind, notify::EventKind::Access(_)) => {
                Some(Message::Paths(e.paths))
            }
            Err(e) => Some(Message::WatchError(e.to_string())),
            _ => None,
        };
        if let Some(message) = message {
            let _ = watcher_tx.send(message);
        }
    })?;
    watcher.watch(
        path.parent().context("config has no parent")?,
        RecursiveMode::NonRecursive,
    )?;
    let mut watched = HashSet::new();
    add_watches(&mut watcher, &config, &mut watched, &events);
    let mut schedules = HashMap::new();
    update_schedules(&config, &mut schedules, 0, true)?;
    let epoch = Instant::now();
    let mut active = HashSet::new();
    let mut queue: VecDeque<(String, u64, Task)> = VecDeque::new();
    let mut manual: HashMap<String, (u64, bool)> = HashMap::new();
    let mut reload_at: Option<u64> = None;
    let mut stopping = false;
    let _ = events.send(ServiceEvent::Config(config.clone()));
    let _ = ready.send(Ok(()));
    loop {
        let now = epoch.elapsed().as_secs();
        if !stopping && reload_at.is_some_and(|due| due <= now) {
            reload_at = None;
            match Config::load(&path) {
                Ok(new) => {
                    update_schedules(&new, &mut schedules, now, false)?;
                    add_watches(&mut watcher, &new, &mut watched, &events);
                    config = new;
                    let _ = events.send(ServiceEvent::Config(config.clone()));
                }
                Err(e) => {
                    let _ = events.send(ServiceEvent::Warning(format!(
                        "invalid config; keeping last valid settings: {e:#}"
                    )));
                }
            }
        }
        if stopping && active.is_empty() {
            break;
        }
        while !stopping && active.len() < config.app.max_parallel {
            let queued = queue.iter().position(|(id, _, _)| !active.contains(id));
            let work = if let Some(position) = queued {
                queue.remove(position)
            } else {
                config.folders.iter().find_map(|folder| {
                    if active.contains(&folder.id) {
                        return None;
                    }
                    if !schedules.get_mut(&folder.id)?.start(now) {
                        return None;
                    }
                    let (request, confirm_initial) =
                        manual.remove(&folder.id).unwrap_or((0, false));
                    Some((folder.id.clone(), request, Task::Sync { confirm_initial }))
                })
            };
            let Some((id, request, task)) = work else {
                break;
            };
            if config.folder(&id).is_err() {
                let _ = events.send(ServiceEvent::Failed {
                    id,
                    request,
                    error: "文件夹已移除".into(),
                    kind: FailureKind::Configuration,
                });
                continue;
            }
            active.insert(id.clone());
            let _ = events.send(ServiceEvent::Started {
                id: id.clone(),
                request,
                task: task.clone(),
            });
            let tx = tx.clone();
            let events = events.clone();
            let path = path.clone();
            let task_config = config.clone();
            thread::spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    execute_task(&path, &task_config, &id, &task, &events)
                }))
                .unwrap_or_else(|_| {
                    Err(anyhow::anyhow!(
                        "background task panicked; local history is preserved"
                    ))
                })
                .map_err(|error| (tide_git::failure_kind(&error), format!("{error:#}")));
                let _ = tx.send(Message::Done {
                    id,
                    request,
                    task,
                    result: Box::new(result),
                });
            });
        }
        let next = schedules
            .iter()
            .filter(|(id, _)| !active.contains(*id))
            .filter_map(|(_, s)| s.next())
            .chain(reload_at)
            .min();
        let timeout = if stopping || active.len() >= config.app.max_parallel {
            Duration::from_secs(1)
        } else {
            next.map(|due| Duration::from_secs(due.saturating_sub(epoch.elapsed().as_secs())))
                .unwrap_or(Duration::from_secs(86400))
        };
        match rx.recv_timeout(timeout) {
            Ok(Message::Run { id, request, task }) if !stopping => {
                if task.is_sync() {
                    match config.folder(&id) {
                        Ok(folder) if !folder.paused => {
                            let Task::Sync { confirm_initial } = task else {
                                unreachable!()
                            };
                            manual.insert(folder.id.clone(), (request, confirm_initial));
                            let s = schedules.get_mut(&folder.id).unwrap();
                            s.retry_stopped = false;
                            s.disabled = false;
                            s.request(epoch.elapsed().as_secs());
                        }
                        _ => {
                            let _ = events.send(ServiceEvent::Failed {
                                id,
                                request,
                                error: "文件夹已暂停或已移除".into(),
                                kind: FailureKind::Configuration,
                            });
                        }
                    }
                } else {
                    queue.push_back((id, request, task));
                }
            }
            Ok(Message::Reload) => reload_at = Some(epoch.elapsed().as_secs()),
            Ok(Message::Wake) => {
                for s in schedules.values_mut() {
                    s.request(epoch.elapsed().as_secs());
                }
            }
            Ok(Message::Stop) => {
                stopping = true;
                queue.clear();
            }
            Ok(Message::Paths(paths)) if !stopping => {
                if paths.iter().any(|p| p == &path) {
                    reload_at = Some(epoch.elapsed().as_secs() + 1);
                }
                for folder in &config.folders {
                    if folder.on_change
                        && !folder.paused
                        && paths.iter().any(|p| relevant_path(p, &folder.path))
                    {
                        schedules
                            .get_mut(&folder.id)
                            .unwrap()
                            .change(epoch.elapsed().as_secs());
                        let _ = events.send(ServiceEvent::Changed {
                            id: folder.id.clone(),
                        });
                    }
                }
            }
            Ok(Message::Done {
                id,
                request,
                task,
                result,
            }) => {
                let result = *result;
                active.remove(&id);
                if let Some(s) = schedules.get_mut(&id) {
                    if task.is_sync() {
                        let (success, stop) = match &result {
                            Ok(Payload::Sync(run)) => (
                                run.success,
                                run.failure_kind.is_some_and(FailureKind::stops_retry)
                                    || run.report.as_ref().is_some_and(|r| r.pending),
                            ),
                            Err((kind, _)) => (false, kind.stops_retry()),
                            _ => (true, false),
                        };
                        s.finish(epoch.elapsed().as_secs(), success, stop);
                    } else if task.changes_documents() && result.is_ok() {
                        s.retry_stopped = false;
                        s.disabled = config.folder(&id).is_ok_and(|f| f.paused);
                        s.request(epoch.elapsed().as_secs());
                    }
                }
                match result {
                    Ok(payload) => {
                        let _ = events.send(ServiceEvent::Completed {
                            id,
                            request,
                            payload,
                        });
                    }
                    Err((kind, error)) => {
                        let _ = events.send(ServiceEvent::Failed {
                            id,
                            request,
                            error,
                            kind,
                        });
                    }
                }
            }
            Ok(Message::WatchError(error)) => {
                let _ = events.send(ServiceEvent::Warning(error));
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
            _ => (),
        }
    }
    Ok(())
}
fn relevant_path(path: &Path, folder: &Path) -> bool {
    path.strip_prefix(folder).is_ok_and(|relative| {
        !relative
            .components()
            .any(|c| c.as_os_str().eq_ignore_ascii_case(".git"))
    })
}

fn execute_task(
    path: &Path,
    last_valid: &Config,
    id: &str,
    task: &Task,
    events: &Sender<ServiceEvent>,
) -> Result<Payload> {
    // An invalid editor save must not invalidate the coordinator's retained
    // settings. Prefer any newer valid pause/removal before starting a task.
    let config = Config::load(path).unwrap_or_else(|_| last_valid.clone());
    if let Task::Sync { confirm_initial } = task {
        let observer = events.clone();
        let id = id.to_string();
        let folder = id.clone();
        return Ok(Payload::Sync(crate::engine::sync_with_config(
            path,
            &config,
            &folder,
            *confirm_initial,
            move |phase| {
                let _ = observer.send(ServiceEvent::Phase {
                    id: id.clone(),
                    phase,
                });
            },
        )?));
    }
    if matches!(task, Task::Runs) {
        return Ok(Payload::Runs(
            runs(path)?
                .into_iter()
                .filter(|r| r.folder_id == id)
                .take(50)
                .collect(),
        ));
    }
    if matches!(task, Task::Recover) {
        tide_git::LibgitBackend::open_for_recovery(config.folder(id)?.options(&config.app))?
            .recover_keep_worktree()?;
        return Ok(Payload::Done);
    }
    let mut git = tide_git::LibgitBackend::open(config.folder(id)?.options(&config.app))?;
    match task {
        Task::Changes => Ok(Payload::Changes(git.changes()?)),
        Task::Plan => Ok(Payload::Plan(git.plan()?)),
        Task::History { skip } => Ok(Payload::History(git.history(*skip, 50)?)),
        Task::Conflicts => Ok(Payload::Conflicts(git.pending_conflicts()?)),
        Task::Diff { revision, path } => Ok(Payload::Diff(git.diff(revision, path.as_deref())?)),
        Task::Restore { revision, path } => {
            git.restore(revision, path.as_deref())?;
            Ok(Payload::Done)
        }
        Task::Overwrite {
            use_local,
            confirm_initial,
        } => {
            git.overwrite(*use_local, *confirm_initial)?;
            Ok(Payload::Done)
        }
        Task::Resolve { path, local } => {
            git.resolve(path, *local)?;
            Ok(Payload::Done)
        }
        Task::Export {
            revision,
            path,
            destination,
        } => {
            git.export(revision, path, destination)?;
            Ok(Payload::Done)
        }
        _ => unreachable!(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn unrelated_reload_preserves_failure_stop_until_explicit_resume() {
        let temp = tempfile::tempdir().unwrap();
        let folder = crate::Folder::new("notes".into(), temp.path().join("notes"), "x".into());
        let id = folder.id.clone();
        let mut config = Config {
            folders: vec![folder],
            ..Default::default()
        };
        let mut schedules = HashMap::new();
        update_schedules(&config, &mut schedules, 0, true).unwrap();
        schedules.get_mut(&id).unwrap().finish(10, false, true);
        config.app.theme = crate::Theme::Dark;
        update_schedules(&config, &mut schedules, 20, false).unwrap();
        assert_eq!(schedules[&id].next(), None);
        config.folders[0].paused = true;
        update_schedules(&config, &mut schedules, 30, false).unwrap();
        config.folders[0].paused = false;
        update_schedules(&config, &mut schedules, 40, false).unwrap();
        assert_eq!(schedules[&id].next(), Some(50));
    }
    #[test]
    fn changing_timer_interval_rearms_and_disables_deadlines() {
        let temp = tempfile::tempdir().unwrap();
        let mut folder = crate::Folder::new("notes".into(), temp.path().join("notes"), "x".into());
        folder.interval = "0s".into();
        let id = folder.id.clone();
        let mut config = Config {
            folders: vec![folder],
            ..Default::default()
        };
        let mut schedules = HashMap::new();
        update_schedules(&config, &mut schedules, 0, true).unwrap();
        assert_eq!(schedules[&id].due, None);
        config.folders[0].interval = "15m".into();
        update_schedules(&config, &mut schedules, 20, false).unwrap();
        assert_eq!(schedules[&id].due, Some(920));
        config.folders[0].interval = "0s".into();
        update_schedules(&config, &mut schedules, 30, false).unwrap();
        assert_eq!(schedules[&id].due, None);
    }
}
