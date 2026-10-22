use crate::update;
use anyhow::{Context, Result};
use slint::{ComponentHandle, ModelRc, VecModel};
use std::{
    cell::RefCell,
    collections::{HashMap, HashSet},
    hash::{Hash, Hasher},
    path::PathBuf,
    rc::Rc,
    time::Duration,
};
use tide_core::{Config, Folder, Notification, Payload, Service, ServiceEvent, Task, Theme};
use tide_git::{ConflictPolicy, FailureKind, HistoryEntry};
slint::include_modules!();

thread_local! {
    static CONTROLLER: RefCell<Option<Rc<RefCell<Controller>>>> = const { RefCell::new(None) };
    static WINDOW: RefCell<Option<MainWindow>> = const { RefCell::new(None) };
}
struct FolderState {
    label: String,
    detail: String,
    color: slint::Color,
    synced: bool,
    confirm: bool,
}
impl Default for FolderState {
    fn default() -> Self {
        Self {
            label: "等待同步".into(),
            detail: "改动会在本地保留，连接成功后自动同步。".into(),
            color: color(117, 163, 177),
            synced: false,
            confirm: false,
        }
    }
}
struct Controller {
    path: PathBuf,
    config: Config,
    service: Service,
    ui: Option<slint::Weak<MainWindow>>,
    selected: String,
    tab: i32,
    request: u64,
    view_request: u64,
    detail_request: u64,
    busy: HashSet<String>,
    states: HashMap<String, FolderState>,
    history: Vec<HistoryEntry>,
    history_skip: usize,
    tray: Option<tray_icon::TrayIcon>,
    tray_update: Option<tray_icon::menu::MenuItem>,
    update: UpdateState,
}
#[derive(Default)]
struct UpdateState {
    checking: bool,
    status: String,
    /// Verified installer ready to run.
    ready: Option<ReadyUpdate>,
    /// The last automatic check failed; retry sooner than the daily interval.
    failed: bool,
    /// Release page for a newer version that this (portable) copy cannot install itself.
    page: Option<String>,
    last_check: Option<std::time::Instant>,
    notified: bool,
    /// Installer to start after the event loop ends and the sync service has stopped,
    /// and whether Tide restarts in the tray only (the window was closed).
    pending_install: Option<(ReadyUpdate, bool)>,
}
// Updates are produced only on Windows (installer builds).
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Clone)]
struct ReadyUpdate {
    version: String,
    path: PathBuf,
    sha256: String,
}
#[cfg_attr(not(windows), allow(dead_code))]
enum UpdateEvent {
    UpToDate,
    Available(update::Release),
    Downloading(update::Release),
    Ready(update::Release, update::Download),
    /// `retry_soon`: the feed was unreachable (offline, proxy). Download or checksum
    /// failures of a release wait for the next daily or manual check instead.
    Failed {
        error: String,
        retry_soon: bool,
    },
}
fn color(r: u8, g: u8, b: u8) -> slint::Color {
    slint::Color::from_rgb_u8(r, g, b)
}
fn model<T: Clone + 'static>(rows: Vec<T>) -> ModelRc<T> {
    Rc::new(VecModel::from(rows)).into()
}
impl Controller {
    fn ui(&self) -> Option<MainWindow> {
        self.ui.as_ref().and_then(slint::Weak::upgrade)
    }
    fn enqueue(&mut self, task: Task, view: bool) {
        if self.selected.is_empty() {
            return;
        }
        self.request += 1;
        if view {
            self.view_request = self.request;
        }
        if matches!(task, Task::Diff { .. }) {
            self.detail_request = self.request;
        }
        self.service.run(self.selected.clone(), self.request, task);
    }
    fn refresh(&mut self) {
        let Some(ui) = self.ui() else {
            return;
        };
        self.render();
        if self.selected.is_empty() {
            ui.set_rows(model(vec![]));
            ui.set_empty_title("添加第一个同步文件夹".into());
            ui.set_empty_detail("本地文件夹与 Git 仓库关联后，修改就有了历史。".into());
            return;
        }
        ui.set_selected_key("".into());
        ui.set_rows(model(vec![]));
        ui.set_empty_title("正在读取…".into());
        ui.set_empty_detail("".into());
        let task = match self.tab {
            0 => Task::Changes,
            1 => Task::History { skip: 0 },
            2 => Task::Conflicts,
            4 => Task::Runs,
            _ => {
                self.load_rules();
                return;
            }
        };
        if self.tab == 1 {
            self.history.clear();
            self.history_skip = 0;
        }
        self.enqueue(task, true);
    }
    fn render(&mut self) {
        let rows = self
            .config
            .folders
            .iter()
            .map(|folder| {
                let state = self.states.entry(folder.id.clone()).or_default();
                FolderRow {
                    id: folder.id.clone().into(),
                    name: folder.name.clone().into(),
                    subtitle: folder.branch.clone().into(),
                    state: if folder.paused {
                        "已暂停".into()
                    } else {
                        state.label.clone().into()
                    },
                    state_color: if folder.paused {
                        color(112, 123, 132)
                    } else {
                        state.color
                    },
                    paused: folder.paused,
                }
            })
            .collect();
        if let Some(ui) = self.ui() {
            ui.set_folders(model(rows));
            ui.set_selected_id(self.selected.clone().into());
            ui.set_tab(self.tab);
            ui.set_theme_mode(match self.config.app.theme {
                Theme::System => 0,
                Theme::Light => 1,
                Theme::Dark => 2,
            });
            if !ui.get_settings_open() {
                ui.set_device_name(self.config.app.device_name.clone().into());
                ui.set_close_to_tray(self.config.app.close_to_tray);
                ui.set_autostart(self.config.app.autostart);
                ui.set_auto_update(self.config.app.auto_update);
                ui.set_theme_index(match self.config.app.theme {
                    Theme::System => 0,
                    Theme::Light => 1,
                    Theme::Dark => 2,
                });
                ui.set_notification_index(match self.config.app.notify {
                    Notification::Errors => 0,
                    Notification::Every => 1,
                    Notification::Off => 2,
                });
            }
            ui.set_footer(
                format!(
                    "{} · {} 个文件夹",
                    self.config.app.device_name,
                    self.config.folders.len()
                )
                .into(),
            );
            if let Ok(folder) = self.config.folder(&self.selected) {
                ui.set_folder_name(folder.name.clone().into());
                ui.set_folder_path(folder.path.display().to_string().into());
                ui.set_remote(format!("{}  ·  {}", folder.remote, folder.branch).into());
                ui.set_paused(folder.paused);
                ui.set_busy(self.busy.contains(&folder.id));
                let state = self.states.get(&folder.id).unwrap();
                ui.set_status(if folder.paused {
                    "已暂停".into()
                } else {
                    state.label.clone().into()
                });
                ui.set_status_detail(state.detail.clone().into());
                ui.set_status_color(state.color);
                ui.set_can_confirm(state.confirm);
            } else {
                ui.set_folder_name("你的同步空间".into());
                ui.set_folder_path("".into());
                ui.set_remote("".into());
                ui.set_status("添加文件夹，开始保留每一次修改。".into());
                ui.set_status_detail("".into());
                ui.set_can_confirm(false);
                ui.set_busy(false);
            }
        }
        self.render_update();
        if let Some(tray) = &self.tray {
            let summary = if !self.busy.is_empty() {
                "正在同步"
            } else if self.states.values().any(|s| s.label == "出错") {
                "有文件夹需要处理"
            } else {
                "文档同步"
            };
            let _ = tray.set_tooltip(Some(format!("Tide · {summary}")));
        }
    }
    fn render_update(&self) {
        let Some(ui) = self.ui() else {
            return;
        };
        ui.set_app_version(update::current_version().to_string().into());
        ui.set_update_status(self.update.status.clone().into());
        ui.set_update_checking(self.update.checking);
        ui.set_update_ready(
            self.update
                .ready
                .as_ref()
                .map(|ready| ready.version.clone())
                .unwrap_or_default()
                .into(),
        );
        ui.set_update_page(self.update.page.is_some());
    }
    /// `manual` checks report every outcome; automatic ones stay quiet unless an update is ready.
    fn start_update_check(&mut self, manual: bool) {
        if self.update.checking || self.update.ready.is_some() {
            self.render_update();
            return;
        }
        self.update.last_check = Some(std::time::Instant::now());
        #[cfg(windows)]
        {
            self.update.checking = true;
            self.update.status = "正在检查更新…".into();
            std::thread::spawn(move || {
                let send = |event: UpdateEvent| {
                    let _ = slint::invoke_from_event_loop(move || {
                        with_controller(|c| c.update_event(event, manual))
                    });
                };
                match update::check() {
                    Ok(None) => send(UpdateEvent::UpToDate),
                    Ok(Some(release)) if update::installed_dir().is_none() => {
                        send(UpdateEvent::Available(release))
                    }
                    Ok(Some(release)) => {
                        send(UpdateEvent::Downloading(release.clone()));
                        match update::download(&release) {
                            Ok(download) => send(UpdateEvent::Ready(release, download)),
                            Err(error) => send(UpdateEvent::Failed {
                                error: format!("{error:#}"),
                                retry_soon: false,
                            }),
                        }
                    }
                    Err(error) => send(UpdateEvent::Failed {
                        error: format!("{error:#}"),
                        retry_soon: true,
                    }),
                }
            });
        }
        #[cfg(not(windows))]
        {
            let _ = manual;
            self.update.status = "自动更新目前只支持 Windows 安装版。".into();
        }
        self.render_update();
    }
    #[cfg_attr(not(windows), allow(dead_code))]
    fn update_event(&mut self, event: UpdateEvent, manual: bool) {
        let checked = chrono::Local::now().format("%H:%M");
        self.update.failed = matches!(
            event,
            UpdateEvent::Failed {
                retry_soon: true,
                ..
            }
        );
        match event {
            UpdateEvent::UpToDate => {
                self.update.checking = false;
                self.update.page = None;
                self.update.status = format!(
                    "已是最新版本 {} · {checked} 检查",
                    update::current_version()
                );
            }
            UpdateEvent::Available(release) => {
                self.update.checking = false;
                self.update.status = format!(
                    "发现新版本 {}。这是便携版，请到发布页下载后替换。",
                    release.version
                );
                self.update.page = Some(release.page);
            }
            UpdateEvent::Downloading(release) => {
                self.update.status = format!("发现新版本 {}，正在下载并校验…", release.version);
            }
            UpdateEvent::Ready(release, download) => {
                self.update.checking = false;
                self.update.status = format!(
                    "{} 已下载并通过校验。重启 Tide 即可完成更新，正在进行的同步会先完成。",
                    release.version
                );
                if let Some(item) = &self.tray_update {
                    item.set_text(format!("重启以更新到 {}", release.version));
                }
                if !self.update.notified && self.config.app.notify != Notification::Off {
                    self.update.notified = true;
                    let body = format!(
                        "Tide {} 已准备好。在托盘菜单或“关于”里重启即可更新。",
                        release.version
                    );
                    std::thread::spawn(move || {
                        let _ = notify_rust::Notification::new()
                            .summary("Tide 有新版本")
                            .body(&body)
                            .show();
                    });
                }
                self.update.ready = Some(ReadyUpdate {
                    version: release.version,
                    path: download.path,
                    sha256: download.sha256,
                });
            }
            UpdateEvent::Failed { error, .. } => {
                self.update.checking = false;
                self.update.status = format!("检查更新失败：{error}");
                if manual {
                    self.error(format!("检查更新失败：{error}"));
                }
            }
        }
        self.render_update();
    }
    /// Close the window and quit; `run` starts the installer once syncing has stopped.
    fn apply_update(&mut self) {
        let Some(ready) = self.update.ready.clone() else {
            return;
        };
        let background = self.ui().is_none();
        self.update.pending_install = Some((ready, background));
        // Deferred like close_requested: this may run inside the window's own click handler.
        let _ = slint::invoke_from_event_loop(|| {
            WINDOW.with(|slot| {
                if let Some(ui) = slot.borrow_mut().take() {
                    let _ = ui.hide();
                }
            });
            with_controller(|c| c.ui = None);
            let _ = slint::quit_event_loop();
        });
    }
    fn load_rules(&self) {
        if let (Some(ui), Ok(f)) = (self.ui(), self.config.folder(&self.selected)) {
            ui.set_interval(f.interval.clone().into());
            ui.set_on_change(f.on_change);
            ui.set_policy_index(match f.conflict {
                ConflictPolicy::KeepBoth => 0,
                ConflictPolicy::PreferLocal => 1,
                ConflictPolicy::PreferRemote => 2,
                ConflictPolicy::Ask => 3,
            });
            ui.set_ignores(f.ignore.join("\n").into());
        }
    }
    fn error(&self, error: impl ToString) {
        if let Some(ui) = self.ui() {
            ui.set_banner(error.to_string().into());
        }
    }
    fn reload(&mut self) {
        match Config::load(&self.path) {
            Ok(config) => {
                self.config = config;
                self.service.reload();
                self.refresh();
            }
            Err(e) => self.error(e),
        }
    }
    fn event(&mut self, event: ServiceEvent) {
        match event {
            ServiceEvent::Config(config) => {
                self.config = config;
                if self.config.folder(&self.selected).is_err() {
                    self.selected = self
                        .config
                        .folders
                        .first()
                        .map(|f| f.id.clone())
                        .unwrap_or_default();
                }
                self.refresh();
            }
            ServiceEvent::Started { id, task, .. } => {
                if matches!(task, Task::Sync { .. }) {
                    self.busy.insert(id.clone());
                    let s = self.states.entry(id).or_default();
                    s.label = "同步中".into();
                    s.detail = "正在收集改动".into();
                    s.color = color(106, 185, 200);
                }
            }
            ServiceEvent::Phase { id, phase } => {
                self.states.entry(id).or_default().detail = phase.label().into();
            }
            ServiceEvent::Changed { id } => {
                if !self.busy.contains(&id) {
                    let s = self.states.entry(id).or_default();
                    s.label = "有改动".into();
                    s.color = color(217, 180, 107);
                }
            }
            ServiceEvent::Completed {
                id,
                request,
                payload,
            } => match payload {
                Payload::Sync(run) => {
                    self.busy.remove(&id);
                    let s = self.states.entry(id.clone()).or_default();
                    s.synced = run.success;
                    s.confirm = run.failure_kind == Some(FailureKind::NeedsConfirmation);
                    if run.success {
                        s.label = "已同步".into();
                        s.color = color(86, 185, 139);
                        s.detail = format!(
                            "{} · {:.2} 秒",
                            chrono::Local::now().format("%H:%M"),
                            run.elapsed_ms as f64 / 1000.0
                        );
                        if let Some(report) = &run.report {
                            if report.changed + report.uploaded > 0 {
                                s.detail = format!(
                                    "{} · 上传 {} 个 · 下载 {} 个",
                                    chrono::Local::now().format("%H:%M"),
                                    report.uploaded,
                                    report.changed
                                );
                            }
                            if !report.conflicts.is_empty() {
                                s.detail =
                                    format!("两份版本已保留 · {} 个冲突", report.conflicts.len());
                            }
                            if !report.mirror_errors.is_empty() {
                                s.detail = "主远端已同步，备份远端需要处理".into();
                            }
                        }
                    } else if run.report.as_ref().is_some_and(|r| r.pending) {
                        s.label = "有冲突".into();
                        s.detail = "请选择每个文件要保留的版本。".into();
                        s.color = color(224, 173, 93);
                    } else {
                        s.label = if run.failure_kind == Some(FailureKind::Network) {
                            "离线"
                        } else {
                            "出错"
                        }
                        .into();
                        s.detail = run.error.clone().unwrap_or_default();
                        s.color = color(225, 119, 109);
                    }
                    if self.config.app.notify == Notification::Every
                        || (!run.success && self.config.app.notify == Notification::Errors)
                    {
                        let body = s.detail.clone();
                        std::thread::spawn(move || {
                            let _ = notify_rust::Notification::new()
                                .summary("Tide")
                                .body(&body)
                                .show();
                        });
                    }
                    if id == self.selected {
                        self.refresh();
                    }
                }
                Payload::Plan(plan) if id == self.selected => {
                    if let Some(ui) = self.ui() {
                        ui.set_banner(format!("本地 {} 个文件（{:.1} MB），远端 {} 个文件（{:.1} MB）。同名不同内容 {} 项。",plan.local_files,plan.local_bytes as f64/1e6,plan.remote_files,plan.remote_bytes as f64/1e6,plan.different_at_same_path.len()).into());
                        ui.set_can_confirm(plan.needs_initial_confirmation);
                    }
                    self.states.entry(id).or_default().confirm = plan.needs_initial_confirmation;
                }
                Payload::Changes(changes)
                    if id == self.selected && request == self.view_request =>
                {
                    let rows = changes
                        .iter()
                        .map(|c| DetailRow {
                            key: c.path.clone().into(),
                            title: c.path.clone().into(),
                            detail: "本地修改 · 等待下一轮同步".into(),
                            tag: c.kind.clone().into(),
                        })
                        .collect();
                    if changes.is_empty() {
                        let s = self.states.entry(id).or_default();
                        if s.synced && s.label == "有改动" {
                            s.label = "已同步".into();
                            s.color = color(86, 185, 139);
                        }
                    }
                    if let Some(ui) = self.ui() {
                        ui.set_rows(model(rows));
                        ui.set_empty_title("没有待同步的改动".into());
                        ui.set_empty_detail("新的修改会显示在这里。".into());
                    }
                }
                Payload::History(entries)
                    if id == self.selected && request == self.view_request =>
                {
                    let more = entries.len() == 50;
                    self.history.extend(entries);
                    self.history_skip = self.history.len();
                    let rows = self
                        .history
                        .iter()
                        .map(|entry| DetailRow {
                            key: entry.id.clone().into(),
                            title: history_title(&entry.message).into(),
                            detail: if entry.local_only {
                                format!("{} · {} · 仅保存在本机", entry.device, &entry.id[..8])
                            } else {
                                format!("{} · {}", entry.device, &entry.id[..8])
                            }
                            .into(),
                            tag: chrono::DateTime::from_timestamp(entry.timestamp, 0)
                                .map(|d| {
                                    d.with_timezone(&chrono::Local)
                                        .format("%m-%d %H:%M")
                                        .to_string()
                                })
                                .unwrap_or_default()
                                .into(),
                        })
                        .collect();
                    if let Some(ui) = self.ui() {
                        ui.set_rows(model(rows));
                        ui.set_more_history(more);
                        ui.set_empty_title("还没有历史版本".into());
                        ui.set_empty_detail("第一次同步后，版本会保留在这里。".into());
                    }
                }
                Payload::Conflicts(conflicts)
                    if id == self.selected && request == self.view_request =>
                {
                    let rows = conflicts
                        .iter()
                        .map(|c| DetailRow {
                            key: c.path.clone().into(),
                            title: c.path.clone().into(),
                            detail: format!("本地 {} · 远端 {}", &c.local[..8], &c.remote[..8])
                                .into(),
                            tag: "待选择".into(),
                        })
                        .collect();
                    if let Some(ui) = self.ui() {
                        ui.set_rows(model(rows));
                        ui.set_empty_title("没有待处理的冲突".into());
                        ui.set_empty_detail("“两份都保留”产生的副本可直接在文件夹里查看。".into());
                    }
                }
                Payload::Runs(runs) if id == self.selected && request == self.view_request => {
                    let rows = runs
                        .iter()
                        .map(|r| DetailRow {
                            key: r.run_id.clone().into(),
                            title: if r.success {
                                "同步完成"
                            } else {
                                "需要处理"
                            }
                            .into(),
                            detail: r
                                .error
                                .clone()
                                .unwrap_or_else(|| {
                                    format!("耗时 {} ms · {}", r.elapsed_ms, r.run_id)
                                })
                                .into(),
                            tag: r.started.chars().take(19).collect::<String>().into(),
                        })
                        .collect();
                    if let Some(ui) = self.ui() {
                        ui.set_rows(model(rows));
                        ui.set_empty_title("还没有同步记录".into());
                        ui.set_empty_detail("每轮同步结果都会显示在这里。".into());
                    }
                }
                Payload::Diff(text) if id == self.selected && request == self.detail_request => {
                    if let Some(ui) = self.ui() {
                        ui.set_diff_text(text.into());
                        ui.set_diff_open(true);
                    }
                }
                Payload::Done if id == self.selected => {
                    self.error("操作已完成。");
                    self.refresh();
                }
                _ => (),
            },
            ServiceEvent::Failed {
                id,
                request,
                error,
                kind,
            } => {
                self.busy.remove(&id);
                let s = self.states.entry(id.clone()).or_default();
                s.label = "出错".into();
                s.detail = error.clone();
                s.color = color(225, 119, 109);
                s.confirm = kind == FailureKind::NeedsConfirmation;
                if id == self.selected
                    && (request == self.view_request || request == self.detail_request)
                {
                    self.error(error);
                }
            }
            ServiceEvent::Warning(error) => self.error(error),
        }
        self.render();
    }
}

pub fn run(path: PathBuf, background: bool, smoke_output: Option<PathBuf>) -> Result<()> {
    if !path.exists() {
        Config::init(&path)?;
    }
    let mut hash = std::collections::hash_map::DefaultHasher::new();
    let canonical = path.canonicalize()?;
    #[cfg(windows)]
    canonical.to_string_lossy().to_lowercase().hash(&mut hash);
    #[cfg(not(windows))]
    canonical.hash(&mut hash);
    let instance = single_instance::SingleInstance::new(&format!("Tide-{:x}", hash.finish()))?;
    if !instance.is_single() {
        if !request_activation(&path) {
            activate_existing();
        }
        return Ok(());
    }
    slint::BackendSelector::new()
        .backend_name("winit".into())
        .renderer_name("software".into())
        .select()?;
    let _activation = start_activation(&path)?;
    let config = Config::load(&path)?;
    let (service, events) = Service::start(path.clone())?;
    let selected = config
        .folders
        .first()
        .map(|f| f.id.clone())
        .unwrap_or_default();
    let controller = Rc::new(RefCell::new(Controller {
        path,
        config,
        service: service.clone(),
        ui: None,
        selected,
        tab: 0,
        request: 0,
        view_request: 0,
        detail_request: 0,
        busy: HashSet::new(),
        states: HashMap::new(),
        history: vec![],
        history_skip: 0,
        tray: None,
        tray_update: None,
        update: UpdateState::default(),
    }));
    CONTROLLER.with(|slot| *slot.borrow_mut() = Some(controller.clone()));
    #[cfg(windows)]
    let (tray, tray_update) = match make_tray() {
        Ok((tray, item)) => (Some(tray), Some(item)),
        Err(_) => (None, None),
    };
    #[cfg(not(windows))]
    let (tray, tray_update) = (None, None);
    controller.borrow_mut().tray = tray;
    controller.borrow_mut().tray_update = tray_update;
    if !background || controller.borrow().tray.is_none() {
        show_window()?;
    }
    std::thread::spawn(move || {
        for event in events {
            if slint::invoke_from_event_loop(move || with_controller(|c| c.event(event))).is_err() {
                break;
            }
        }
    });
    // Automatic update check: about a minute after start, then daily while running;
    // a failed check (offline, proxy down) is retried after an hour.
    let update_timer = slint::Timer::default();
    if smoke_output.is_none() {
        update_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_secs(60),
            move || {
                with_controller(|c| {
                    let hours = if c.update.failed { 1 } else { 24 };
                    let due = c
                        .update
                        .last_check
                        .is_none_or(|last| last.elapsed() >= Duration::from_secs(hours * 3600));
                    if c.config.app.auto_update && due {
                        c.start_update_check(false);
                    }
                })
            },
        );
    }
    let smoke_timer = slint::Timer::default();
    let smoke_failed = Rc::new(std::cell::Cell::new(false));
    if let Some(output) = smoke_output {
        let failed = smoke_failed.clone();
        let step = std::cell::Cell::new(0usize);
        smoke_timer.start(
            slint::TimerMode::Repeated,
            Duration::from_millis(700),
            move || {
                let names = [
                    "desktop",
                    "settings",
                    "add-folder",
                    "about",
                    "history",
                    "rules",
                    "light",
                    "conflicts",
                ];
                let index = step.get();
                let result = WINDOW.with(|slot| -> Result<()> {
                    let window = slot.borrow();
                    let ui = window.as_ref().context("no desktop window")?;
                    let pixels = ui.window().take_snapshot()?;
                    std::fs::create_dir_all(&output)?;
                    image::save_buffer(
                        output.join(format!("{}.png", names[index])),
                        pixels.as_bytes(),
                        pixels.width(),
                        pixels.height(),
                        image::ColorType::Rgba8,
                    )?;
                    match index {
                        0 => ui.set_settings_open(true),
                        1 => {
                            ui.set_settings_open(false);
                            ui.set_add_open(true);
                        }
                        2 => {
                            ui.set_add_open(false);
                            ui.set_about_open(true);
                        }
                        3 => {
                            ui.set_about_open(false);
                            ui.invoke_switch_tab(1);
                        }
                        4 => ui.invoke_switch_tab(3),
                        5 => ui.set_theme_mode(1),
                        6 => {
                            ui.set_theme_mode(2);
                            ui.invoke_switch_tab(2);
                        }
                        _ => (),
                    }
                    Ok(())
                });
                if let Err(error) = result {
                    eprintln!("UI smoke failed: {error:#}");
                    failed.set(true);
                }
                step.set(index + 1);
                if index + 1 == names.len() || failed.get() {
                    let _ = slint::quit_event_loop();
                }
            },
        );
    }
    slint::run_event_loop_until_quit()?;
    drop(update_timer);
    // Waits for the running sync task, so the installer never replaces files mid-sync.
    let stopped = service.shutdown();
    let pending_install = CONTROLLER.with(|slot| {
        slot.borrow()
            .as_ref()
            .and_then(|c| c.borrow_mut().update.pending_install.take())
    });
    WINDOW.with(|slot| slot.borrow_mut().take());
    CONTROLLER.with(|slot| slot.borrow_mut().take());
    drop(instance);
    // The user asked to update: start setup even if stopping the service reported an error;
    // the process is exiting either way and no further writes happen from here.
    if let Some((ready, background)) = pending_install {
        start_installer(ready, background);
    }
    stopped?;
    anyhow::ensure!(!smoke_failed.get(), "UI smoke capture failed");
    Ok(())
}
#[cfg(windows)]
fn start_installer(ready: ReadyUpdate, background: bool) {
    let download = update::Download {
        path: ready.path,
        sha256: ready.sha256,
    };
    if let Err(error) =
        update::launch_installer(&download, update::InstallMode::Silent { background })
    {
        rfd::MessageDialog::new()
            .set_title("Tide 更新没有开始")
            .set_description(format!(
                "{error:#}\n\n可以重新打开 Tide 再试，或到 {}/releases 手动下载安装包。",
                update::REPOSITORY
            ))
            .set_level(rfd::MessageLevel::Error)
            .show();
    }
}
#[cfg(not(windows))]
fn start_installer(_: ReadyUpdate, _: bool) {}
fn history_title(message: &str) -> String {
    let short = |id: &str| id.chars().take(8).collect::<String>();
    match message {
        "Tide: local snapshot" => "自动保存".into(),
        "Tide: reconcile devices" => "合并设备改动".into(),
        "Tide: replace remote from local" => "用本地版本覆盖远端".into(),
        "Tide: replace local from remote" => "用远端版本覆盖本地".into(),
        "Tide: replaced local version" => "被远端替换前的本地版本".into(),
        "Tide: preserve worktree after interrupted write" => "保留中断时的文件夹内容".into(),
        _ => match message.strip_prefix("Tide: restore ") {
            Some(id) => format!("恢复到 {} 的版本", short(id)),
            None => message.into(),
        },
    }
}
fn with_controller(action: impl FnOnce(&mut Controller)) {
    CONTROLLER.with(|slot| {
        if let Some(controller) = slot.borrow().as_ref() {
            action(&mut controller.borrow_mut());
        }
    });
}
fn show_window() -> Result<()> {
    if WINDOW.with(|slot| {
        if let Some(ui) = slot.borrow().as_ref() {
            let _ = ui.show();
            true
        } else {
            false
        }
    }) {
        return Ok(());
    }
    let ui = MainWindow::new()?;
    ui.window().set_size(slint::LogicalSize::new(1100., 730.));
    ui.on_matches_filter(|query, title, detail| {
        let query = query.to_lowercase();
        query.is_empty()
            || title.to_lowercase().contains(&query)
            || detail.to_lowercase().contains(&query)
    });
    with_controller(|c| c.ui = Some(ui.as_weak()));
    ui.on_select_folder(|id| {
        with_controller(|c| {
            c.selected = id.to_string();
            c.refresh();
        })
    });
    ui.on_switch_tab(|tab| {
        with_controller(|c| {
            c.tab = tab;
            c.refresh();
        })
    });
    ui.on_sync(|confirm| {
        with_controller(|c| {
            c.enqueue(
                Task::Sync {
                    confirm_initial: confirm,
                },
                false,
            )
        })
    });
    ui.on_sync_all(|| {
        with_controller(|c| {
            for f in &c.config.folders {
                if !f.paused {
                    c.service.run(
                        f.id.clone(),
                        0,
                        Task::Sync {
                            confirm_initial: false,
                        },
                    );
                }
            }
        })
    });
    ui.on_toggle_pause(|| {
        with_controller(|c| {
            let id = c.selected.clone();
            let result = Config::edit(&c.path, |config| {
                let f = config
                    .folders
                    .iter_mut()
                    .find(|f| f.id == id)
                    .context("文件夹已移除")?;
                f.paused = !f.paused;
                Ok(())
            });
            if let Err(e) = result {
                c.error(e);
            } else {
                c.reload();
            }
        })
    });
    ui.on_open_folder(|| {
        with_controller(|c| {
            if let Ok(f) = c.config.folder(&c.selected) {
                let _ = open::that_detached(&f.path);
            }
        })
    });
    ui.on_open_config(|| {
        with_controller(|c| {
            let _ = open::that_detached(&c.path);
        })
    });
    ui.on_browse_folder(|| {
        if let Some(path) = rfd::FileDialog::new().pick_folder() {
            with_controller(|c| {
                if let Some(ui) = c.ui() {
                    ui.set_add_path(path.display().to_string().into());
                }
            });
        }
    });
    ui.on_add_folder(|| {
        with_controller(|c| {
            let Some(ui) = c.ui() else {
                return;
            };
            let result = (|| -> Result<String> {
                let folder_path = PathBuf::from(ui.get_add_path().as_str()).canonicalize()?;
                let mut folder = Folder::new(
                    ui.get_add_name().to_string(),
                    folder_path,
                    ui.get_add_remote().to_string(),
                );
                folder.branch = ui.get_add_branch().trim().to_string();
                let id = folder.id.clone();
                Config::edit(&c.path, |config| {
                    config.folders.push(folder);
                    Ok(())
                })?;
                Ok(id)
            })();
            match result {
                Ok(id) => {
                    ui.set_add_open(false);
                    ui.set_add_error("".into());
                    c.selected = id;
                    c.reload();
                    c.enqueue(Task::Plan, false);
                }
                Err(e) => ui.set_add_error(format!("{e:#}").into()),
            }
        })
    });
    ui.on_save_rules(|| {
        with_controller(|c| {
            let Some(ui) = c.ui() else {
                return;
            };
            let result = Config::edit(&c.path, |config| {
                let f = config
                    .folders
                    .iter_mut()
                    .find(|f| f.id == c.selected)
                    .context("请先选择文件夹")?;
                f.interval = ui.get_interval().trim().into();
                f.on_change = ui.get_on_change();
                f.conflict = match ui.get_policy_index() {
                    1 => ConflictPolicy::PreferLocal,
                    2 => ConflictPolicy::PreferRemote,
                    3 => ConflictPolicy::Ask,
                    _ => ConflictPolicy::KeepBoth,
                };
                f.ignore = ui
                    .get_ignores()
                    .lines()
                    .filter(|l| !l.trim().is_empty())
                    .map(str::to_string)
                    .collect();
                Ok(())
            });
            match result {
                Ok(()) => {
                    c.reload();
                    c.error("规则已保存");
                }
                Err(e) => c.error(e),
            }
        })
    });
    ui.on_save_settings(|| with_controller(save_settings));
    ui.on_edit_settings(|| {
        with_controller(|c| {
            if let Some(ui) = c.ui() {
                ui.set_settings_open(false);
                c.render();
                ui.set_settings_open(true);
            }
        })
    });
    ui.on_export_preferences(|| {
        let mut preferences = None;
        with_controller(|c| preferences = Some(tide_core::Preferences::from_config(&c.config)));
        if let (Some(preferences), Some(destination)) = (
            preferences,
            rfd::FileDialog::new()
                .set_file_name("tide-preferences.toml")
                .add_filter("TOML", &["toml"])
                .save_file(),
        ) {
            let result = preferences.export(&destination);
            with_controller(|c| match result {
                Ok(()) => {
                    if let Some(ui) = c.ui() {
                        ui.set_settings_open(false);
                    }
                    c.error("偏好已导出，可放入同步文件夹。另一设备需手动导入。");
                }
                Err(error) => {
                    if let Some(ui) = c.ui() {
                        ui.set_add_error(format!("{error:#}").into());
                    }
                }
            });
        }
    });
    ui.on_import_preferences(|| {
        let Some(source) = rfd::FileDialog::new().add_filter("TOML", &["toml"]).pick_file() else { return; };
        let result = tide_core::Preferences::load(&source);
        let preferences = match result {
            Ok(preferences) => preferences,
            Err(error) => { with_controller(|c| { if let Some(ui) = c.ui() { ui.set_add_error(format!("{error:#}").into()); } }); return; }
        };
        let theme = match preferences.theme { Theme::System => "跟随系统", Theme::Light => "浅色", Theme::Dark => "深色" };
        let notify = match preferences.notify { Notification::Errors => "仅错误通知", Notification::Every => "每次同步通知", Notification::Off => "关闭通知" };
        let confirmed = rfd::MessageDialog::new().set_title("导入 Tide 偏好")
            .set_description(format!("主题：{theme}\n通知：{notify}\n\n只应用这两项偏好。设备名、路径、凭据、同步规则和开机启动保持本机设置。"))
            .set_buttons(rfd::MessageButtons::OkCancel).show();
        if confirmed == rfd::MessageDialogResult::Ok {
            with_controller(|c| match preferences.apply(&c.path) {
                Ok(()) => { if let Some(ui) = c.ui() { ui.set_settings_open(false); } c.reload(); c.error("主题和通知偏好已导入。"); }
                Err(error) => { if let Some(ui) = c.ui() { ui.set_add_error(format!("{error:#}").into()); } }
            });
        }
    });
    ui.on_remove_folder(|| {
        with_controller(|c| {
            let id = c.selected.clone();
            match Config::edit(&c.path, |config| {
                config.folders.retain(|f| f.id != id);
                Ok(())
            }) {
                Ok(()) => c.reload(),
                Err(e) => c.error(e),
            }
        })
    });
    ui.on_plan(|| with_controller(|c| c.enqueue(Task::Plan, false)));
    ui.on_load_more(|| {
        with_controller(|c| {
            c.enqueue(
                Task::History {
                    skip: c.history_skip,
                },
                true,
            )
        })
    });
    ui.on_inspect_row(|key| {
        with_controller(|c| {
            if c.tab == 1 {
                c.enqueue(
                    Task::Diff {
                        revision: key.to_string(),
                        path: None,
                    },
                    false,
                );
            }
        })
    });
    ui.on_restore(|| {
        with_controller(|c| {
            if let Some(ui) = c.ui() {
                let revision = ui.get_selected_key().to_string();
                if !revision.is_empty() {
                    c.enqueue(
                        Task::Restore {
                            revision,
                            path: None,
                        },
                        false,
                    );
                }
            }
        })
    });
    ui.on_resolve(|local| {
        with_controller(|c| {
            if let Some(ui) = c.ui() {
                let path = ui.get_selected_key().to_string();
                if !path.is_empty() {
                    c.enqueue(Task::Resolve { path, local }, false);
                }
            }
        })
    });
    ui.on_overwrite(|use_local| {
        with_controller(|c| {
            c.enqueue(
                Task::Overwrite {
                    use_local,
                    confirm_initial: false,
                },
                false,
            )
        })
    });
    ui.on_recover(|| with_controller(|c| c.enqueue(Task::Recover, false)));
    ui.on_export_file(|| {
        with_controller(|c| {
            if let Some(ui) = c.ui() {
                ui.set_export_folder(c.selected.clone().into());
                ui.set_export_revision(ui.get_selected_key());
                ui.set_export_open(true);
            }
        })
    });
    ui.on_export_selected(|| {
        // Native dialogs may pump the event loop: release all RefCell borrows first.
        let selection = CONTROLLER.with(|slot| {
            slot.borrow().as_ref().and_then(|controller| {
                let c = controller.borrow();
                let ui = c.ui()?;
                Some((
                    ui.get_export_folder().to_string(),
                    ui.get_export_revision().to_string(),
                    ui.get_export_path().to_string(),
                ))
            })
        });
        if let Some((id, revision, path)) = selection {
            let filename = path.rsplit('/').next().unwrap_or("export");
            if let Some(destination) = rfd::FileDialog::new().set_file_name(filename).save_file() {
                with_controller(|c| {
                    if let Some(ui) = c.ui() {
                        ui.set_export_open(false);
                    }
                    c.request += 1;
                    c.service.run(
                        id,
                        c.request,
                        Task::Export {
                            revision,
                            path,
                            destination,
                        },
                    );
                });
            }
        }
    });
    ui.on_check_update(|| with_controller(|c| c.start_update_check(true)));
    ui.on_apply_update(|| with_controller(Controller::apply_update));
    ui.on_open_release(|| {
        with_controller(|c| {
            if let Some(page) = &c.update.page {
                let _ = open::that_detached(page);
            }
        })
    });
    ui.on_quit(|| {
        let _ = slint::quit_event_loop();
    });
    ui.window().on_close_requested(|| {
        let hide = CONTROLLER.with(|slot| {
            slot.borrow().as_ref().is_some_and(|c| {
                let c = c.borrow();
                c.config.app.close_to_tray && c.tray.is_some()
            })
        });
        if hide {
            let _ = slint::invoke_from_event_loop(|| {
                WINDOW.with(|slot| {
                    if let Some(ui) = slot.borrow_mut().take() {
                        let _ = ui.hide();
                    }
                });
                with_controller(|c| c.ui = None);
            });
        } else {
            let _ = slint::quit_event_loop();
        }
        slint::CloseRequestResponse::KeepWindowShown
    });
    ui.show()?;
    WINDOW.with(|slot| *slot.borrow_mut() = Some(ui));
    with_controller(Controller::refresh);
    Ok(())
}
fn save_settings(c: &mut Controller) {
    let Some(ui) = c.ui() else {
        return;
    };
    let mut app = c.config.app.clone();
    app.device_name = ui.get_device_name().trim().into();
    app.close_to_tray = ui.get_close_to_tray();
    app.autostart = ui.get_autostart();
    app.auto_update = ui.get_auto_update();
    app.theme = match ui.get_theme_index() {
        1 => Theme::Light,
        2 => Theme::Dark,
        _ => Theme::System,
    };
    app.notify = match ui.get_notification_index() {
        1 => Notification::Every,
        2 => Notification::Off,
        _ => Notification::Errors,
    };
    let result = (|| -> Result<()> {
        let mut validated = c.config.clone();
        validated.app = app.clone();
        validated.validate()?;
        if app.autostart != c.config.app.autostart {
            let exe = std::env::current_exe()?;
            let config_path = c.path.display().to_string();
            let launch = auto_launch::AutoLaunchBuilder::new()
                .set_app_name("Tide")
                .set_app_path(exe.to_str().context("non-UTF8 executable path")?)
                .set_args(&["--config", &config_path, "gui", "--background"])
                .build()?;
            if app.autostart {
                launch.enable()?;
            } else {
                launch.disable()?;
            }
        }
        Config::edit(&c.path, |config| {
            config.app = app;
            Ok(())
        })?;
        Ok(())
    })();
    match result {
        Ok(()) => {
            ui.set_settings_open(false);
            ui.set_add_error("".into());
            c.reload();
        }
        Err(e) => ui.set_add_error(format!("{e:#}").into()),
    }
}
#[cfg(windows)]
fn make_tray() -> Result<(tray_icon::TrayIcon, tray_icon::menu::MenuItem)> {
    use tray_icon::{
        menu::{Menu, MenuEvent, MenuItem},
        TrayIconBuilder, TrayIconEvent,
    };
    let menu = Menu::new();
    let update_item = MenuItem::with_id("update", "检查更新", true, None);
    for (id, text) in [
        ("show", "打开 Tide"),
        ("sync", "全部同步"),
        ("pause", "暂停全部"),
        ("resume", "继续全部"),
    ] {
        menu.append(&MenuItem::with_id(id, text, true, None))?;
    }
    menu.append(&update_item)?;
    menu.append(&MenuItem::with_id("quit", "退出", true, None))?;
    MenuEvent::set_event_handler(Some(|event: MenuEvent| {
        let id = event.id.0;
        let _ = slint::invoke_from_event_loop(move || match id.as_str() {
            "show" => {
                let _ = show_window();
            }
            "sync" => with_controller(|c| {
                for f in &c.config.folders {
                    if !f.paused {
                        c.service.run(
                            f.id.clone(),
                            0,
                            Task::Sync {
                                confirm_initial: false,
                            },
                        );
                    }
                }
            }),
            "pause" | "resume" => with_controller(|c| {
                let paused = id == "pause";
                if let Err(e) = Config::edit(&c.path, |config| {
                    for f in &mut config.folders {
                        f.paused = paused;
                    }
                    Ok(())
                }) {
                    c.error(e);
                } else {
                    c.reload();
                }
            }),
            "update" => {
                let ready = CONTROLLER.with(|slot| {
                    slot.borrow()
                        .as_ref()
                        .is_some_and(|c| c.borrow().update.ready.is_some())
                });
                if ready {
                    with_controller(Controller::apply_update);
                } else {
                    // Show the result where it is reported: the About dialog.
                    let _ = show_window();
                    with_controller(|c| {
                        if let Some(ui) = c.ui() {
                            ui.set_about_open(true);
                        }
                        c.start_update_check(true);
                    });
                }
            }
            "quit" => {
                let _ = slint::quit_event_loop();
            }
            _ => (),
        });
    }));
    TrayIconEvent::set_event_handler(Some(|event| {
        if matches!(
            event,
            TrayIconEvent::Click {
                button: tray_icon::MouseButton::Left,
                button_state: tray_icon::MouseButtonState::Up,
                ..
            }
        ) {
            let _ = slint::invoke_from_event_loop(|| {
                let _ = show_window();
            });
        }
    }));
    let icon =
        image::load_from_memory(include_bytes!("../../../assets/brand/tide-tray.png"))?.to_rgba8();
    let (width, height) = icon.dimensions();
    let tray = TrayIconBuilder::new()
        .with_menu(Box::new(menu))
        .with_tooltip("Tide · 文档同步")
        .with_icon(tray_icon::Icon::from_rgba(icon.into_raw(), width, height)?)
        .build()?;
    Ok((tray, update_item))
}
fn activate_existing() {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::*;
        let title: Vec<u16> = "Tide\0".encode_utf16().collect();
        let window = FindWindowW(std::ptr::null(), title.as_ptr());
        if !window.is_null() {
            ShowWindow(window, SW_RESTORE);
            SetForegroundWindow(window);
        }
    }
}

#[derive(serde::Deserialize, serde::Serialize)]
struct ActivationEndpoint {
    port: u16,
    token: String,
}
struct ActivationFile(PathBuf);
impl Drop for ActivationFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}
fn start_activation(path: &std::path::Path) -> Result<ActivationFile> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))?;
    let endpoint = ActivationEndpoint {
        port: listener.local_addr()?.port(),
        token: uuid::Uuid::new_v4().to_string(),
    };
    let file = path.with_extension("activation.json");
    let mut temp =
        tempfile::NamedTempFile::new_in(path.parent().context("missing config directory")?)?;
    serde_json::to_writer(&mut temp, &endpoint)?;
    temp.flush()?;
    temp.persist(&file).map_err(|e| e.error)?;
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else {
                break;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
            let mut token = String::new();
            if std::io::Read::by_ref(&mut stream)
                .take(64)
                .read_to_string(&mut token)
                .is_ok()
                && token == endpoint.token
            {
                let _ = slint::invoke_from_event_loop(|| {
                    let _ = show_window();
                });
            }
        }
    });
    Ok(ActivationFile(file))
}
fn request_activation(path: &std::path::Path) -> bool {
    use std::io::Write;
    let result = (|| -> Result<()> {
        let endpoint: ActivationEndpoint =
            serde_json::from_reader(std::fs::File::open(path.with_extension("activation.json"))?)?;
        let address = std::net::SocketAddr::from((std::net::Ipv4Addr::LOCALHOST, endpoint.port));
        let mut stream = std::net::TcpStream::connect_timeout(&address, Duration::from_secs(1))?;
        stream.write_all(endpoint.token.as_bytes())?;
        stream.shutdown(std::net::Shutdown::Write)?;
        Ok(())
    })();
    result.is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use i_slint_backend_testing::ElementHandle;
    use slint::Model;
    #[test]
    fn real_controls_receive_pointer_clicks_and_respect_disabled_state() {
        i_slint_backend_testing::init_no_event_loop();
        let ui = MainWindow::new().unwrap();
        ui.window().set_size(slint::LogicalSize::new(1100., 730.));
        ui.set_selected_id("f-test".into());
        ui.set_theme_mode(2);
        let count = Rc::new(std::cell::Cell::new(0));
        let clicked = count.clone();
        ui.on_sync(move |_| clicked.set(clicked.get() + 1));
        ui.show().unwrap();
        let sync = ElementHandle::find_by_accessible_label(&ui, "立即同步")
            .next()
            .unwrap();
        sync.mock_single_click(slint::platform::PointerEventButton::Left);
        assert_eq!(count.get(), 1);
        ui.set_paused(true);
        sync.mock_single_click(slint::platform::PointerEventButton::Left);
        assert_eq!(count.get(), 1);
        ElementHandle::find_by_accessible_label(&ui, "+")
            .next()
            .unwrap()
            .mock_single_click(slint::platform::PointerEventButton::Left);
        assert!(ui.get_add_open());
        ElementHandle::find_by_accessible_label(&ui, "关闭")
            .next()
            .unwrap()
            .mock_single_click(slint::platform::PointerEventButton::Left);
        assert!(!ui.get_add_open());
        for text in [slint::platform::Key::Control.into(), "n".into()] {
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::KeyPressed { text });
        }
        for text in ["n".into(), slint::platform::Key::Control.into()] {
            ui.window()
                .dispatch_event(slint::platform::WindowEvent::KeyReleased { text });
        }
        assert!(ui.get_add_open());
        ui.window()
            .dispatch_event(slint::platform::WindowEvent::KeyPressed {
                text: slint::platform::Key::Escape.into(),
            });
        assert!(!ui.get_add_open());
        ui.hide().unwrap();
    }
    #[test]
    fn outdated_folder_response_does_not_replace_current_history() {
        i_slint_backend_testing::init_no_event_loop();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("config.toml");
        Config::init(&path).unwrap();
        let (service, events) = Service::start(path.clone()).unwrap();
        let ui = MainWindow::new().unwrap();
        let mut config = Config::default();
        let mut folder = Folder::new("notes".into(), temp.path().join("notes"), "x".into());
        folder.id = "f-test".into();
        config.folders.push(folder);
        let mut c = Controller {
            path,
            config,
            service: service.clone(),
            ui: Some(ui.as_weak()),
            selected: "f-test".into(),
            tab: 1,
            request: 21,
            view_request: 21,
            detail_request: 0,
            busy: HashSet::new(),
            states: HashMap::new(),
            history: vec![],
            history_skip: 0,
            tray: None,
            tray_update: None,
            update: UpdateState::default(),
        };
        let entry = HistoryEntry {
            id: "0123456789abcdef".into(),
            device: "device".into(),
            timestamp: 0,
            message: "test".into(),
            changed: 1,
            local_only: false,
        };
        c.event(ServiceEvent::Completed {
            id: "f-test".into(),
            request: 20,
            payload: Payload::History(vec![entry.clone()]),
        });
        assert!(c.history.is_empty());
        c.event(ServiceEvent::Completed {
            id: "f-other".into(),
            request: 21,
            payload: Payload::History(vec![entry.clone()]),
        });
        assert!(c.history.is_empty());
        c.event(ServiceEvent::Completed {
            id: "f-test".into(),
            request: 21,
            payload: Payload::History(vec![entry]),
        });
        assert_eq!(ui.get_rows().row_count(), 1);
        ui.set_settings_open(true);
        ui.set_device_name("unsaved device".into());
        ui.set_theme_index(1);
        ui.set_autostart(true);
        c.event(ServiceEvent::Phase {
            id: "f-test".into(),
            phase: tide_git::SyncPhase::Fetch,
        });
        assert_eq!(ui.get_device_name(), "unsaved device");
        assert_eq!(ui.get_theme_index(), 1);
        assert!(ui.get_autostart());
        service.shutdown().unwrap();
        while events.recv_timeout(Duration::from_secs(5)).is_ok() {}
    }
}
