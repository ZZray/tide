use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    path::{Path, PathBuf},
    time::Duration,
};
use tide_git::{ConflictPolicy, RepoOptions};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub app: App,
    #[serde(default, rename = "folder")]
    pub folders: Vec<Folder>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct App {
    pub device_name: String,
    pub max_parallel: usize,
    pub theme: Theme,
    pub close_to_tray: bool,
    pub autostart: bool,
    pub notify: Notification,
    /// Check GitHub Releases daily; installer builds also download verified updates,
    /// portable copies only report the new version.
    /// Written only when disabled, so configurations stay readable by older versions.
    #[serde(skip_serializing_if = "is_true")]
    pub auto_update: bool,
}
fn is_true(value: &bool) -> bool {
    *value
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Theme {
    #[default]
    System,
    Light,
    Dark,
}
#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Notification {
    #[default]
    Errors,
    Every,
    Off,
}
impl Default for App {
    fn default() -> Self {
        Self {
            device_name: std::env::var("COMPUTERNAME")
                .or_else(|_| std::env::var("HOSTNAME"))
                .unwrap_or_else(|_| "My computer".into()),
            max_parallel: 2,
            theme: Theme::System,
            close_to_tray: true,
            autostart: false,
            notify: Notification::Errors,
            auto_update: true,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Folder {
    pub id: String,
    pub name: String,
    pub path: PathBuf,
    #[serde(default)]
    pub git_dir: Option<PathBuf>,
    pub remote: String,
    #[serde(default = "default_branch")]
    pub branch: String,
    #[serde(default)]
    pub mirrors: Vec<String>,
    #[serde(default)]
    pub ssh_key: Option<PathBuf>,
    #[serde(default = "default_interval")]
    pub interval: String,
    #[serde(default = "yes")]
    pub on_change: bool,
    #[serde(default = "default_debounce")]
    pub debounce: String,
    #[serde(default)]
    pub conflict: ConflictPolicy,
    #[serde(default = "default_ignore")]
    pub ignore: Vec<String>,
    #[serde(default = "default_extensions")]
    pub text_extensions: Vec<String>,
    #[serde(default)]
    pub large_file: LargeFile,
    #[serde(default)]
    pub paused: bool,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct LargeFile {
    pub warn: u64,
    pub reject: u64,
}
impl Default for LargeFile {
    fn default() -> Self {
        Self {
            warn: 50_000_000,
            reject: 100_000_000,
        }
    }
}
fn default_branch() -> String {
    "main".into()
}
fn default_interval() -> String {
    "15m".into()
}
fn default_debounce() -> String {
    "30s".into()
}
fn yes() -> bool {
    true
}
fn default_ignore() -> Vec<String> {
    [
        "~$*",
        "*.tmp",
        ".~lock.*#",
        ".DS_Store",
        "Thumbs.db",
        "desktop.ini",
    ]
    .map(str::to_string)
    .into()
}
fn default_extensions() -> Vec<String> {
    ["md", "txt", "org", "rst", "tex", "csv"]
        .map(str::to_string)
        .into()
}

impl Folder {
    pub fn new(name: String, path: PathBuf, remote: String) -> Self {
        Self {
            id: format!("f-{}", uuid::Uuid::new_v4()),
            name,
            path,
            remote,
            git_dir: None,
            branch: default_branch(),
            mirrors: vec![],
            ssh_key: None,
            interval: default_interval(),
            on_change: true,
            debounce: default_debounce(),
            conflict: ConflictPolicy::KeepBoth,
            ignore: default_ignore(),
            text_extensions: default_extensions(),
            large_file: LargeFile::default(),
            paused: false,
        }
    }
    pub fn options(&self, app: &App) -> RepoOptions {
        let mut options = RepoOptions::new(
            self.path.clone(),
            self.remote.clone(),
            app.device_name.clone(),
        );
        options.git_dir = self.git_dir.clone();
        options.branch = self.branch.clone();
        options.ssh_key = self.ssh_key.clone();
        options.ignore = self.ignore.clone();
        options.policy = self.conflict;
        options.mirrors = self.mirrors.clone();
        options.text_extensions = self.text_extensions.clone();
        options.warn_bytes = self.large_file.warn;
        options.reject_bytes = self.large_file.reject;
        options
    }
}

pub fn duration(input: &str) -> Result<Duration> {
    let (number, multiplier) = if let Some(n) = input.strip_suffix('s') {
        (n, 1)
    } else if let Some(n) = input.strip_suffix('m') {
        (n, 60)
    } else if let Some(n) = input.strip_suffix('h') {
        (n, 3600)
    } else {
        bail!("duration must end in s, m, or h: {input}");
    };
    let seconds = number
        .parse::<u64>()?
        .checked_mul(multiplier)
        .context("duration overflow")?;
    if seconds > 31_536_000 {
        bail!("duration cannot exceed one year");
    }
    Ok(Duration::from_secs(seconds))
}

fn normalized(path: &Path) -> Result<PathBuf> {
    if !path.is_absolute() {
        bail!("use an absolute path: {}", path.display());
    }
    if path.components().any(|c| {
        matches!(
            c,
            std::path::Component::ParentDir | std::path::Component::CurDir
        )
    }) {
        bail!(
            "path must not contain . or .. components: {}",
            path.display()
        );
    }
    // Resolve existing parents so aliases and absent removable drives cannot evade overlap checks.
    let mut existing = path;
    let mut tail = Vec::new();
    while !existing.exists() {
        let Some(name) = existing.file_name() else {
            // A removable/network volume can be absent. Configuration must still
            // load so the engine can persist its missing-folder pause.
            #[cfg(windows)]
            return Ok(PathBuf::from(path.to_string_lossy().to_lowercase()));
            #[cfg(not(windows))]
            return Ok(path.to_path_buf());
        };
        tail.push(name.to_owned());
        existing = existing.parent().context("path has no existing ancestor")?;
    }
    let mut result = existing.canonicalize()?;
    for part in tail.into_iter().rev() {
        result.push(part);
    }
    #[cfg(windows)]
    {
        result = PathBuf::from(result.to_string_lossy().to_lowercase());
    }
    Ok(result)
}

impl Config {
    pub fn default_path() -> Result<PathBuf> {
        Ok(directories::ProjectDirs::from("org", "Tide", "Tide")
            .context("cannot determine user config directory")?
            .config_dir()
            .join("config.toml"))
    }
    pub fn load(path: &Path) -> Result<Self> {
        let config: Self = toml::from_str(
            &fs::read_to_string(path)
                .with_context(|| format!("cannot load {}; run `tide init`", path.display()))?,
        )?;
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        if self.app.device_name.trim().is_empty()
            || self.app.device_name.contains(['\n', '\r', '<', '>'])
        {
            bail!("invalid device name");
        }
        if !(1..=2).contains(&self.app.max_parallel) {
            bail!("max_parallel must be 1 or 2");
        }
        let mut ids = HashSet::new();
        let mut names = HashSet::new();
        let mut paths: Vec<PathBuf> = vec![];
        for folder in &self.folders {
            if folder.id.is_empty()
                || folder.name.trim().is_empty()
                || !ids.insert(&folder.id)
                || !names.insert(&folder.name)
            {
                bail!("folder ids and names must be nonempty and unique");
            }
            if !folder
                .id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-')
            {
                bail!("invalid folder id");
            }
            validate_remote(&folder.remote)?;
            for mirror in &folder.mirrors {
                validate_remote(mirror)?;
            }
            duration(&folder.interval)?;
            if duration(&folder.debounce)?.is_zero() {
                bail!("debounce must be positive");
            }
            if folder.large_file.warn > folder.large_file.reject || folder.large_file.reject == 0 {
                bail!("invalid large-file thresholds");
            }
            let path = normalized(&folder.path)?;
            if paths
                .iter()
                .any(|p| path.starts_with(p) || p.starts_with(&path))
            {
                bail!("sync folders cannot overlap or be nested");
            }
            paths.push(path);
            if let Some(key) = &folder.ssh_key {
                if !key.is_absolute() {
                    bail!("SSH key path must be absolute (expand ~)");
                }
            }
        }
        for folder in &self.folders {
            if folder.name != folder.id && ids.contains(&folder.name) {
                bail!("folder name cannot equal another folder id");
            }
        }
        let mut metadata = Vec::new();
        for folder in &self.folders {
            if let Some(dir) = &folder.git_dir {
                let dir = normalized(dir)?;
                if paths
                    .iter()
                    .any(|p| dir.starts_with(p) || p.starts_with(&dir))
                    || metadata
                        .iter()
                        .any(|p: &PathBuf| dir.starts_with(p) || p.starts_with(&dir))
                {
                    bail!("external Git directories must not overlap any sync folder or another Git directory");
                }
                metadata.push(dir);
            }
        }
        Ok(())
    }
    pub fn folder(&self, name: &str) -> Result<&Folder> {
        self.folders
            .iter()
            .find(|f| f.id == name)
            .or_else(|| self.folders.iter().find(|f| f.name == name))
            .context("unknown folder name or id")
    }
    fn save(&self, path: &Path) -> Result<()> {
        self.validate()?;
        let parent = path
            .parent()
            .context("config must have a parent directory")?;
        fs::create_dir_all(parent)?;
        let mut temp = tempfile::NamedTempFile::new_in(parent)?;
        use std::io::Write;
        temp.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        temp.as_file().sync_all()?;
        temp.persist(path).map_err(|e| e.error)?;
        Ok(())
    }
    pub fn init(path: &Path) -> Result<()> {
        if path.exists() {
            bail!("config already exists; it was not overwritten");
        }
        Self::edit(path, |config| {
            if path.exists() {
                bail!("config already exists; it was not overwritten");
            }
            *config = Self::default();
            Ok(())
        })
    }
    pub fn edit(path: &Path, edit: impl FnOnce(&mut Self) -> Result<()>) -> Result<()> {
        fs::create_dir_all(path.parent().context("config has no parent")?)?;
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path.with_extension("lock"))?;
        lock.try_lock_exclusive()
            .context("config is being edited by another process")?;
        let mut config = if path.exists() {
            Self::load(path)?
        } else {
            Self::default()
        };
        edit(&mut config)?;
        config.save(path)
    }
}

pub fn validate_remote(remote: &str) -> Result<()> {
    if remote.trim().is_empty() || remote.contains(['\n', '\r', '\0']) {
        bail!("remote is empty or invalid");
    }
    // Embedded HTTPS tokens would leak into config/logs. Use the system credential helper.
    if let Some((scheme, rest)) = remote.split_once("://") {
        let authority = rest.split('/').next().unwrap_or("");
        if rest.contains(['?', '#'])
            || authority
                .rsplit_once('@')
                .is_some_and(|(user, _)| user.contains(':'))
        {
            bail!("do not put passwords or query tokens in remote URLs; use credentials outside config");
        }
        if matches!(scheme, "http" | "https") && authority.contains('@') {
            bail!("do not put credentials in URLs; use the Git credential helper");
        }
        if !matches!(scheme, "https" | "http" | "ssh" | "file" | "git") {
            bail!("unsupported remote URL scheme");
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn durations_are_bounded() {
        assert_eq!(duration("15m").unwrap().as_secs(), 900);
        assert!(duration("0s").unwrap().is_zero());
        assert!(duration("-1m").is_err());
        assert!(duration("99999999999999999h").is_err());
    }
    #[test]
    fn id_lookup_cannot_be_shadowed_by_another_name() {
        let temp = tempfile::tempdir().unwrap();
        let mut a = Folder::new("f-b".into(), temp.path().join("a"), "x".into());
        a.id = "f-a".into();
        let mut b = Folder::new("B".into(), temp.path().join("b"), "x".into());
        b.id = "f-b".into();
        let config = Config {
            folders: vec![a, b],
            ..Default::default()
        };
        assert_eq!(config.folder("f-b").unwrap().id, "f-b");
        assert!(config.validate().is_err());
    }
    #[test]
    fn roundtrip_and_overlap() {
        let temp = tempfile::tempdir().unwrap();
        let p = temp.path().join("config.toml");
        Config::init(&p).unwrap();
        Config::edit(&p, |c| {
            c.folders.push(Folder::new(
                "notes".into(),
                temp.path().join("notes"),
                "git@example.com:notes.git".into(),
            ));
            Ok(())
        })
        .unwrap();
        let mut c = Config::load(&p).unwrap();
        assert_eq!(c.folders.len(), 1);
        c.folders.push(Folder::new(
            "nested".into(),
            temp.path().join("notes/sub"),
            "x".into(),
        ));
        assert!(c.validate().is_err());
        assert!(Config::init(&p).is_err());
    }
    #[test]
    fn auto_update_defaults_on_and_is_written_only_when_disabled() {
        let parsed: Config = toml::from_str("[app]\ntheme = \"dark\"\n").unwrap();
        assert!(parsed.app.auto_update);
        assert!(!toml::to_string(&parsed).unwrap().contains("auto_update"));
        let mut off = parsed.clone();
        off.app.auto_update = false;
        let text = toml::to_string(&off).unwrap();
        assert!(text.contains("auto_update = false"));
        assert!(!toml::from_str::<Config>(&text).unwrap().app.auto_update);
    }
    #[test]
    fn reject_embedded_tokens() {
        assert!(validate_remote("https://user:secret@github.com/me/repo").is_err());
        assert!(validate_remote("git@github.com:me/repo.git").is_ok());
    }
}
