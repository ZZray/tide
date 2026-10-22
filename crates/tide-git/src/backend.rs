use crate::{
    reconcile, validate_files, Conflict, ConflictPolicy, Files, ReconcileOptions, Version,
};
use crate::{ChangeEntry, FailureKind, SyncFailure, SyncPhase};
#[cfg(test)]
#[path = "backend_tests.rs"]
mod tests;
use anyhow::{bail, Context, Result};
use chrono::Local;
use fs2::FileExt;
use git2::{
    Cred, ErrorCode, FetchOptions, Index, IndexEntry, IndexTime, Oid, PushOptions, RemoteCallbacks,
    Repository, RepositoryInitOptions, RepositoryState, Signature, Status, StatusOptions,
};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{Duration, SystemTime},
};

#[derive(Clone, Debug)]
pub struct RepoOptions {
    pub path: PathBuf,
    pub git_dir: Option<PathBuf>,
    pub remote: String,
    pub branch: String,
    pub device: String,
    pub ssh_key: Option<PathBuf>,
    pub ignore: Vec<String>,
    pub text_extensions: Vec<String>,
    pub policy: ConflictPolicy,
    pub stable_for: Duration,
    pub warn_bytes: u64,
    pub reject_bytes: u64,
    pub mirrors: Vec<String>,
}

impl RepoOptions {
    pub fn new(path: PathBuf, remote: String, device: String) -> Self {
        Self {
            path,
            remote,
            device,
            git_dir: None,
            branch: "main".into(),
            ssh_key: None,
            ignore: vec![],
            text_extensions: ["md", "txt", "org", "rst", "tex", "csv"]
                .map(str::to_string)
                .into(),
            policy: ConflictPolicy::KeepBoth,
            stable_for: Duration::from_secs(2),
            warn_bytes: 50_000_000,
            reject_bytes: 100_000_000,
            mirrors: vec![],
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SyncReport {
    pub head: String,
    /// Files added, modified or deleted in the local folder by this round.
    pub changed: usize,
    /// Files added, modified or deleted on the remote branch by this round.
    #[serde(default)]
    pub uploaded: usize,
    pub conflicts: Vec<Conflict>,
    pub warnings: Vec<String>,
    pub mirror_errors: Vec<String>,
    pub pending: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct HistoryEntry {
    pub id: String,
    pub device: String,
    pub timestamp: i64,
    pub message: String,
    pub changed: usize,
    /// Kept only on this device, e.g. a local version replaced by the remote one.
    pub local_only: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct AssociationPlan {
    pub local_files: usize,
    pub local_bytes: u64,
    pub remote_files: usize,
    pub remote_bytes: u64,
    pub different_at_same_path: Vec<String>,
    pub needs_initial_confirmation: bool,
}

/// All network and worktree operations are kept behind this boundary.
pub trait GitBackend {
    fn sync(&mut self, confirm_initial_merge: bool) -> Result<SyncReport>;
    fn history(&self, skip: usize, limit: usize) -> Result<Vec<HistoryEntry>>;
    fn restore(&mut self, revision: &str, path: Option<&str>) -> Result<()>;
}

pub struct LibgitBackend {
    repo: Repository,
    options: RepoOptions,
    _lock: File,
    observer: Option<Box<dyn Fn(SyncPhase) + Send>>,
    #[cfg(test)]
    before_push: Option<Box<dyn FnMut()>>,
}

#[derive(Serialize, Deserialize)]
struct Journal {
    target: String,
    before: BTreeMap<String, Option<String>>,
    #[serde(default)]
    before_modes: BTreeMap<String, u32>,
    #[serde(default)]
    case_renames: BTreeMap<String, String>,
}

/// Paths added, modified or removed between two file sets.
fn count_changes(before: &Files, after: &Files) -> usize {
    before
        .iter()
        .filter(|(p, v)| after.get(*p) != Some(*v))
        .count()
        + after.keys().filter(|p| !before.contains_key(*p)).count()
}

/// libgit2 C-quotes non-ASCII paths in patch headers (git's `core.quotepath`),
/// e.g. `"a/\344\271\246.md"`. Decode quoted segments so headers read as UTF-8.
fn unquote_header(line: &[u8]) -> Vec<u8> {
    let octal = |b: u8| (b'0'..=b'7').contains(&b);
    let mut out = Vec::with_capacity(line.len());
    let mut i = 0;
    while i < line.len() {
        if line[i] != b'"' {
            out.push(line[i]);
            i += 1;
            continue;
        }
        let mut decoded = Vec::new();
        let mut j = i + 1;
        let mut closed = false;
        while j < line.len() {
            match line[j] {
                b'"' => {
                    closed = true;
                    break;
                }
                b'\n' => break,
                b'\\'
                    if j + 3 < line.len()
                        && (b'0'..=b'3').contains(&line[j + 1])
                        && octal(line[j + 2])
                        && octal(line[j + 3]) =>
                {
                    decoded.push(
                        (line[j + 1] - b'0') * 64 + (line[j + 2] - b'0') * 8 + (line[j + 3] - b'0'),
                    );
                    j += 4;
                }
                b'\\' if j + 1 < line.len() => {
                    decoded.push(match line[j + 1] {
                        b'n' => b'\n',
                        b't' => b'\t',
                        b'r' => b'\r',
                        b'a' => 7,
                        b'b' => 8,
                        b'f' => 12,
                        b'v' => 11,
                        other => other,
                    });
                    j += 2;
                }
                byte => {
                    decoded.push(byte);
                    j += 1;
                }
            }
        }
        if closed {
            out.extend_from_slice(&decoded);
            i = j + 1;
        } else {
            // Not a quoted path: keep the original bytes.
            out.push(line[i]);
            i += 1;
        }
    }
    out
}

fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut file = tempfile::NamedTempFile::new_in(path.parent().context("missing parent")?)?;
    serde_json::to_writer_pretty(&mut file, value)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

impl LibgitBackend {
    pub fn open(options: RepoOptions) -> Result<Self> {
        Self::open_inner(options, false)
    }

    pub fn open_for_recovery(options: RepoOptions) -> Result<Self> {
        Self::open_inner(options, true)
    }

    fn open_inner(options: RepoOptions, recovery: bool) -> Result<Self> {
        if !options.path.is_dir() {
            return Err(SyncFailure::new(
                FailureKind::MissingFolder,
                format!(
                    "folder missing; remote is unchanged: {}",
                    options.path.display()
                ),
            )
            .into());
        }
        if !git2::Reference::is_valid_name(&format!("refs/heads/{}", options.branch)) {
            bail!("invalid branch name");
        }
        let repo = if let Some(dir) = &options.git_dir {
            if options.path.join(".git").exists() {
                let existing = Repository::open(&options.path)?;
                if !dir.exists() || existing.path().canonicalize()? != dir.canonicalize()? {
                    bail!("folder already has different Git history; metadata migration requires a separate backup and is not automatic");
                }
            }
            if dir.exists() {
                Repository::open(dir)?
            } else {
                let mut init = RepositoryInitOptions::new();
                init.workdir_path(&options.path)
                    .initial_head(&options.branch)
                    .mkdir(true)
                    .mkpath(true)
                    .no_dotgit_dir(true);
                let repo = Repository::init_opts(dir, &init)?;
                // libgit2 creates a gitlink for an external workdir. This new
                // pointer is ours; keep all metadata outside the user's folder.
                let gitlink = options.path.join(".git");
                if gitlink.is_file() {
                    fs::remove_file(gitlink)?;
                }
                repo
            }
        } else if options.path.join(".git").exists() {
            Repository::open(&options.path)?
        } else {
            let mut init = RepositoryInitOptions::new();
            init.initial_head(&options.branch);
            Repository::init_opts(&options.path, &init)?
        };
        let actual = repo
            .workdir()
            .context("bare repositories cannot be sync folders")?
            .canonicalize()?;
        if actual != options.path.canonicalize()? {
            bail!("Git worktree does not match configured folder");
        }
        let lock = OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(repo.path().join("tide.lock"))?;
        lock.try_lock_exclusive().map_err(|_| {
            SyncFailure::new(
                FailureKind::Busy,
                "folder is already in use by another Tide process",
            )
        })?;
        if repo.state() != RepositoryState::Clean {
            return Err(SyncFailure::new(
                FailureKind::Configuration,
                "finish the existing Git merge/rebase before syncing",
            )
            .into());
        }
        if repo.path().join("index.lock").exists() {
            return Err(SyncFailure::new(
                FailureKind::Busy,
                "Git index.lock exists; skipping this round",
            )
            .into());
        }
        match repo.head() {
            Ok(head) if head.name()? != format!("refs/heads/{}", options.branch) => bail!(
                "configured branch differs from checked-out branch; select the existing branch"
            ),
            Ok(_) => (),
            Err(e) if matches!(e.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => (),
            Err(e) => return Err(e.into()),
        }
        let mut config = repo.config()?;
        config.set_bool("core.autocrlf", false)?;
        #[cfg(windows)]
        config.set_bool("core.longpaths", true)?;
        #[cfg(target_os = "macos")]
        config.set_bool("core.precomposeunicode", true)?;
        drop(config);
        for rule in &options.ignore {
            repo.add_ignore_rule(rule)?;
        }
        let backend = Self {
            repo,
            options,
            _lock: lock,
            observer: None,
            #[cfg(test)]
            before_push: None,
        };
        if !recovery {
            backend.resume_write()?;
        }
        Ok(backend)
    }

    pub fn repository(&self) -> &Repository {
        &self.repo
    }

    pub fn set_observer(&mut self, observer: impl Fn(SyncPhase) + Send + 'static) {
        self.observer = Some(Box::new(observer));
    }
    fn phase(&self, phase: SyncPhase) {
        if let Some(observer) = &self.observer {
            observer(phase);
        }
    }
    pub fn changes(&self) -> Result<Vec<ChangeEntry>> {
        let mut options = StatusOptions::new();
        options.include_untracked(true).recurse_untracked_dirs(true);
        let baseline = self
            .head()?
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        let mut paths: BTreeMap<String, Status> = baseline
            .keys()
            .map(|p| (p.clone(), Status::CURRENT))
            .collect();
        for entry in self.repo.statuses(Some(&mut options))?.iter() {
            paths.insert(
                entry.path().context("non-UTF8 filename")?.into(),
                entry.status(),
            );
        }
        let mut changes = Vec::new();
        for (path, status) in paths {
            let before = baseline.get(&path);
            let current = self.disk_version(&path, before)?;
            let staged = status.intersects(
                Status::INDEX_NEW
                    | Status::INDEX_MODIFIED
                    | Status::INDEX_DELETED
                    | Status::INDEX_RENAMED
                    | Status::INDEX_TYPECHANGE
                    | Status::CONFLICTED,
            );
            if !staged && current.as_ref() == before {
                continue;
            }
            let kind = if status.intersects(Status::CONFLICTED) {
                "冲突"
            } else if current.is_none() {
                "删除"
            } else if before.is_none() {
                "新增"
            } else {
                "修改"
            };
            changes.push(ChangeEntry {
                path,
                kind: kind.into(),
            });
        }
        Ok(changes)
    }

    /// Tests connectivity without committing, pushing, or changing document bytes.
    /// Fetching updates private Git metadata.
    pub fn plan(&self) -> Result<AssociationPlan> {
        let local = self.head()?;
        let mut files = local
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        for change in self.changes()? {
            let path = change.path;
            if let Some(version) = self.disk_version(&path, files.get(&path))? {
                files.insert(path, version);
            } else {
                files.remove(&path);
            }
        }
        let remote = self.fetch()?;
        let remote_files = remote
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        let local_bytes = files
            .keys()
            .map(|path| Ok(fs::metadata(self.checked_path(path)?)?.len()))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .sum();
        let remote_bytes = remote_files
            .values()
            .map(|v| Ok(self.repo.find_blob(v.oid)?.size() as u64))
            .collect::<Result<Vec<_>>>()?
            .into_iter()
            .sum();
        let different = files
            .iter()
            .filter(|(p, v)| remote_files.get(*p).is_some_and(|r| r.oid != v.oid))
            .map(|(p, _)| p.clone())
            .collect();
        let needs_initial_confirmation = match (local, remote) {
            (Some(l), Some(r)) => self.repo.merge_base(l, r).is_err(),
            (None, Some(_)) => !files.is_empty(),
            _ => false,
        };
        Ok(AssociationPlan {
            local_files: files.len(),
            local_bytes,
            remote_files: remote_files.len(),
            remote_bytes,
            different_at_same_path: different,
            needs_initial_confirmation,
        })
    }

    /// Preserve the current worktree as a new commit, retaining both the old HEAD
    /// and the interrupted target as ancestors. Never changes the remote.
    pub fn recover_keep_worktree(&mut self) -> Result<()> {
        let journal_path = self.repo.path().join("tide-write.json");
        let journal: Journal = serde_json::from_reader(
            File::open(&journal_path).context("no interrupted write to recover")?,
        )?;
        let target = Oid::from_str(&journal.target)?;
        let head = self.snapshot(false)?.0;
        let files = head
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        let mut parents = vec![target];
        if let Some(head) = head {
            if head != target {
                parents.push(head);
            }
        }
        let recovered = self.commit(
            &files,
            &parents,
            "Tide: preserve worktree after interrupted write",
        )?;
        self.move_head(recovered)?;
        fs::remove_file(journal_path)?;
        if let Ok(mut reference) = self.repo.find_reference("refs/tide/pending") {
            reference.delete()?;
        }
        Ok(())
    }

    pub fn overwrite(&mut self, use_local: bool, confirm_initial: bool) -> Result<SyncReport> {
        if !self.pending_conflicts()?.is_empty() {
            bail!("resolve pending conflicts before replacing");
        }
        let (local, warnings) = self.snapshot(true)?;
        let remote = self.fetch()?;
        if !use_local && remote.is_none() {
            bail!("remote branch has no content; nothing to replace the local folder with");
        }
        let local_files = local
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        let remote_files = remote
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        if let (Some(l), Some(r)) = (local, remote) {
            if self.repo.merge_base(l, r).is_err() && !confirm_initial {
                bail!("initial association needs review; use --confirm-initial-merge after inspecting both folders");
            }
        }
        let mut parents = Vec::new();
        if let Some(remote) = remote {
            parents.push(remote);
        }
        if use_local {
            if let Some(local) = local {
                if !parents.contains(&local) {
                    parents.push(local);
                }
            }
        } else if let (Some(local), Some(remote)) = (local, remote) {
            // The replaced local version stays on this device only: it is kept under
            // refs/tide/replaced/* (listed in history) but is not a parent, so it is never pushed.
            let published = local == remote || self.repo.graph_descendant_of(remote, local)?;
            if !published {
                self.repo.reference(
                    &format!(
                        "refs/tide/replaced/{}",
                        Local::now().format("%Y%m%d%H%M%S%f")
                    ),
                    local,
                    false,
                    "Tide replaced local snapshot",
                )?;
            }
        }
        let target = match remote {
            // Remote content already exists as a commit: adopt it instead of adding an empty one.
            Some(remote) if !use_local => remote,
            _ => self.commit(&local_files, &parents, "Tide: replace remote from local")?,
        };
        self.install(target, &local_files)?;
        let (changed, uploaded) = if use_local {
            (0, count_changes(&remote_files, &local_files))
        } else {
            (count_changes(&local_files, &remote_files), 0)
        };
        if use_local {
            self.push(&self.options.remote, target).context("remote changed or push failed; local replacement commit is preserved; review and retry")?;
            self.repo
                .reference("refs/tide/synced", target, true, "Tide synchronized")?;
        }
        Ok(SyncReport {
            head: target.to_string(),
            changed,
            uploaded,
            warnings,
            ..Default::default()
        })
    }

    fn head(&self) -> Result<Option<Oid>> {
        match self.repo.head() {
            Ok(h) => Ok(Some(h.peel_to_commit()?.id())),
            Err(e) if matches!(e.code(), ErrorCode::UnbornBranch | ErrorCode::NotFound) => Ok(None),
            Err(e) => Err(e.into()),
        }
    }

    pub fn files_at(&self, commit: Oid) -> Result<Files> {
        let tree = self.repo.find_commit(commit)?.tree()?;
        let mut files = Files::new();
        let mut error = None;
        tree.walk(git2::TreeWalkMode::PreOrder, |root, entry| {
            if entry.kind() == Some(git2::ObjectType::Tree) {
                return 0;
            }
            let Ok(name) = entry.name() else {
                error = Some("non-UTF8 Git filename".to_string());
                return -1;
            };
            let path = format!("{root}{name}");
            if !matches!(entry.filemode(), 0o100644 | 0o100755) {
                error = Some(format!("symlinks and submodules are not supported: {path}"));
                return -1;
            }
            files.insert(
                path,
                Version {
                    oid: entry.id(),
                    mode: entry.filemode() as u32,
                },
            );
            0
        })
        .or_else(|e| if error.is_some() { Ok(()) } else { Err(e) })?;
        if let Some(error) = error {
            bail!(error);
        }
        validate_files(&files)?;
        Ok(files)
    }

    fn tree(&self, files: &Files) -> Result<Oid> {
        validate_files(files)?;
        let mut index = Index::new()?;
        for (path, v) in files {
            index.add(&IndexEntry {
                ctime: IndexTime::new(0, 0),
                mtime: IndexTime::new(0, 0),
                dev: 0,
                ino: 0,
                mode: v.mode,
                uid: 0,
                gid: 0,
                file_size: 0,
                id: v.oid,
                flags: 0,
                flags_extended: 0,
                path: path.as_bytes().to_vec(),
            })?;
        }
        Ok(index.write_tree_to(&self.repo)?)
    }

    fn commit(&self, files: &Files, parents: &[Oid], message: &str) -> Result<Oid> {
        let tree = self.repo.find_tree(self.tree(files)?)?;
        let signature = Signature::now(&self.options.device, "tide@localhost")?;
        let parents = parents
            .iter()
            .map(|id| self.repo.find_commit(*id))
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let refs = parents.iter().collect::<Vec<_>>();
        Ok(self
            .repo
            .commit(None, &signature, &signature, message, &tree, &refs)?)
    }

    fn move_head(&self, id: Oid) -> Result<()> {
        // Worktree and index precede HEAD. Replaying the journal is idempotent.
        let tree = self.repo.find_commit(id)?.tree()?;
        let mut index = self.repo.index()?;
        index.read_tree(&tree)?;
        index.write()?;
        self.repo.reference(
            &format!("refs/heads/{}", self.options.branch),
            id,
            true,
            "Tide update",
        )?;
        Ok(())
    }

    fn checked_path(&self, relative: &str) -> Result<PathBuf> {
        crate::validate_path(relative)?;
        let mut path = self.options.path.clone();
        for part in relative.split('/') {
            path.push(part);
            match fs::symlink_metadata(&path) {
                Ok(meta) if meta.file_type().is_symlink() => {
                    bail!("refusing to follow symlink: {}", path.display())
                }
                // Windows directory junctions also carry FILE_ATTRIBUTE_REPARSE_POINT.
                #[cfg(windows)]
                Ok(meta)
                    if {
                        use std::os::windows::fs::MetadataExt;
                        meta.file_attributes() & 0x400 != 0
                    } =>
                {
                    bail!("refusing reparse point: {}", path.display())
                }
                Ok(_) => (),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => (),
                Err(e) => return Err(e.into()),
            }
        }
        Ok(path)
    }

    fn disk_oid(&self, path: &str) -> Result<Option<Oid>> {
        let full = self.checked_path(path)?;
        match fs::read(&full) {
            Ok(bytes) => Ok(Some(Oid::hash_object(git2::ObjectType::Blob, &bytes)?)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e)
                .with_context(|| format!("cannot read {}; no overwrite performed", full.display())),
        }
    }

    fn disk_version(&self, path: &str, baseline: Option<&Version>) -> Result<Option<Version>> {
        let Some(oid) = self.disk_oid(path)? else {
            return Ok(None);
        };
        #[cfg(unix)]
        let mode = {
            use std::os::unix::fs::PermissionsExt;
            let _ = baseline;
            if fs::metadata(self.checked_path(path)?)?.permissions().mode() & 0o111 != 0 {
                0o100755
            } else {
                0o100644
            }
        };
        #[cfg(not(unix))]
        let mode = baseline.map(|v| v.mode).unwrap_or(0o100644);
        Ok(Some(Version { oid, mode }))
    }

    fn snapshot(&self, enforce_guard: bool) -> Result<(Option<Oid>, Vec<String>)> {
        if !self.options.path.is_dir() {
            return Err(SyncFailure::new(
                FailureKind::MissingFolder,
                "folder missing; remote is unchanged",
            )
            .into());
        }
        let head = self.head()?;
        let old = head
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        let mut files = old.clone();
        let mut status_options = StatusOptions::new();
        status_options
            .include_untracked(true)
            .recurse_untracked_dirs(true)
            .renames_head_to_index(false);
        let statuses = self.repo.statuses(Some(&mut status_options))?;
        let mut warnings = Vec::new();
        let mut changed = BTreeSet::new();
        for entry in statuses.iter() {
            if entry.status().intersects(
                Status::INDEX_NEW
                    | Status::INDEX_MODIFIED
                    | Status::INDEX_DELETED
                    | Status::INDEX_RENAMED
                    | Status::INDEX_TYPECHANGE
                    | Status::CONFLICTED,
            ) {
                return Err(SyncFailure::new(
                    FailureKind::Configuration,
                    "existing staged changes or conflicts; commit/unstage them before Tide sync",
                )
                .into());
            }
            changed.insert(
                entry
                    .path()
                    .context("non-UTF8 filename is not supported")?
                    .to_string(),
            );
        }
        // Git status may normalize through attributes or trust a clone's cached
        // stat data. Our baseline must describe actual document bytes, not the
        // filtered view, before any writeback is allowed.
        for (path, version) in &old {
            if !changed.contains(path) && self.disk_version(path, Some(version))? != Some(*version)
            {
                changed.insert(path.clone());
            }
        }
        for path in &changed {
            crate::validate_path(path)?;
            let full = self.checked_path(path)?;
            match fs::metadata(&full) {
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    files.remove(path);
                }
                Err(e) => return Err(e.into()),
                Ok(meta) => {
                    if !meta.is_file() {
                        bail!("only regular files are supported: {path}");
                    }
                    if SystemTime::now()
                        .duration_since(meta.modified()?)
                        .unwrap_or_default()
                        < self.options.stable_for
                    {
                        bail!("file is still being written; retry after 2 seconds: {path}");
                    }
                    if meta.len() > self.options.reject_bytes {
                        bail!("file exceeds size limit; add an ignore rule or raise limit: {path}");
                    }
                    if meta.len() > self.options.warn_bytes {
                        warnings.push(format!("large file: {path} ({} bytes)", meta.len()));
                    }
                    let bytes = fs::read(&full).with_context(|| format!("file is busy: {path}"))?;
                    let after = fs::metadata(&full)?;
                    if meta.len() != after.len() || meta.modified()? != after.modified()? {
                        bail!("file changed while reading: {path}");
                    }
                    let mode = {
                        #[cfg(unix)]
                        {
                            use std::os::unix::fs::PermissionsExt;
                            if meta.permissions().mode() & 0o111 != 0 {
                                0o100755
                            } else {
                                0o100644
                            }
                        }
                        #[cfg(not(unix))]
                        {
                            old.get(path).map(|v| v.mode).unwrap_or(0o100644)
                        }
                    };
                    files.insert(
                        path.into(),
                        Version {
                            oid: self.repo.blob(&bytes)?,
                            mode,
                        },
                    );
                }
            }
        }
        let deleted = old.keys().filter(|p| !files.contains_key(*p)).count();
        if enforce_guard && deleted > 50 && deleted * 100 > old.len() * 30 {
            return Err(SyncFailure::new(FailureKind::DeletionGuard, format!("deletion guard: remote is unchanged; {deleted}/{} files deleted; folder must be paused", old.len())).into());
        }
        if files == old && head.is_some() {
            return Ok((head, warnings));
        }
        if files.is_empty() && head.is_none() {
            return Ok((None, warnings));
        }
        let id = self.commit(
            &files,
            &head.into_iter().collect::<Vec<_>>(),
            "Tide: local snapshot",
        )?;
        self.move_head(id)?;
        Ok((Some(id), warnings))
    }

    fn callbacks(&self) -> RemoteCallbacks<'_> {
        let mut callbacks = RemoteCallbacks::new();
        callbacks.credentials(|url, username, allowed| {
            let acquire = || {
            let ssh_username = username.unwrap_or("git");
            if allowed.contains(git2::CredentialType::SSH_KEY) {
                if let Some(key) = &self.options.ssh_key {
                    return Cred::ssh_key(ssh_username, None, key, None);
                }
                return Cred::ssh_key_from_agent(ssh_username);
            }
            if allowed.contains(git2::CredentialType::USER_PASS_PLAINTEXT) {
                return Cred::credential_helper(&self.repo.config()?, url, username);
            }
            if allowed.contains(git2::CredentialType::USERNAME) {
                return Cred::username(ssh_username);
            }
            Cred::default()
            };
            acquire().map_err(|_| git2::Error::new(ErrorCode::Auth, git2::ErrorClass::Callback, "authentication unavailable; configure a Git credential helper, SSH agent, or an accessible SSH key"))
        });
        callbacks
    }

    fn fetch(&self) -> Result<Option<Oid>> {
        self.phase(SyncPhase::Fetch);
        let mut remote = self.repo.remote_anonymous(&self.options.remote)?;
        let wanted = format!("refs/heads/{}", self.options.branch);
        let (has_branch, has_heads) = {
            let mut proxy = git2::ProxyOptions::new();
            proxy.auto();
            let connection =
                remote.connect_auth(git2::Direction::Fetch, Some(self.callbacks()), Some(proxy))?;
            let refs = connection.list()?;
            (
                refs.iter().any(|r| r.name() == wanted),
                refs.iter().any(|r| r.name().starts_with("refs/heads/")),
            )
        };
        if !has_branch {
            if has_heads {
                return Err(SyncFailure::new(
                    FailureKind::Configuration,
                    format!("remote branch does not exist: {}", self.options.branch),
                )
                .into());
            }
            return Ok(None);
        }
        let mut fetch = FetchOptions::new();
        let mut proxy = git2::ProxyOptions::new();
        proxy.auto();
        fetch.proxy_options(proxy);
        fetch.remote_callbacks(self.callbacks());
        remote.fetch(
            &[&format!("+{wanted}:refs/tide/remote")],
            Some(&mut fetch),
            None,
        )?;
        Ok(Some(self.repo.refname_to_id("refs/tide/remote")?))
    }

    fn push(&self, url: &str, id: Oid) -> Result<()> {
        self.repo
            .reference("refs/tide/push", id, true, "Tide push")?;
        let mut remote = self.repo.remote_anonymous(url)?;
        let mut callbacks = self.callbacks();
        callbacks.push_update_reference(|_, status| match status {
            Some(message) if message.contains("non-fast-forward") => Err(git2::Error::new(
                ErrorCode::NotFastForward,
                git2::ErrorClass::Reference,
                message,
            )),
            Some(message) => Err(git2::Error::from_str(message)),
            None => Ok(()),
        });
        let mut push = PushOptions::new();
        let mut proxy = git2::ProxyOptions::new();
        proxy.auto();
        push.proxy_options(proxy);
        push.remote_callbacks(callbacks);
        remote.push(
            &[&format!(
                "refs/tide/push:refs/heads/{}",
                self.options.branch
            )],
            Some(&mut push),
        )?;
        Ok(())
    }

    fn install(&self, target: Oid, baseline: &Files) -> Result<()> {
        self.phase(SyncPhase::Write);
        let files = self.files_at(target)?;
        let paths: BTreeSet<_> = baseline.keys().chain(files.keys()).cloned().collect();
        let mut before = BTreeMap::new();
        let mut before_modes = BTreeMap::new();
        let mut case_renames = BTreeMap::new();
        for new in files.keys().filter(|path| !baseline.contains_key(*path)) {
            if let Some(old) = baseline
                .keys()
                .find(|old| old.to_lowercase() == new.to_lowercase() && !files.contains_key(*old))
            {
                let source = self.checked_path(old)?;
                let destination = self.checked_path(new)?;
                // Only aliasing paths need component renames. On a case-sensitive
                // volume ordinary file writes/deletes must preserve sibling paths.
                if destination.exists() && same_file::is_same_file(source, destination)? {
                    case_renames.insert(old.clone(), new.clone());
                }
            }
        }
        for path in paths {
            if baseline.get(&path) == files.get(&path) {
                continue;
            }
            if case_renames.contains_key(&path) {
                continue;
            }
            let original_path = case_renames
                .iter()
                .find_map(|(old, new)| if new == &path { Some(old) } else { None })
                .unwrap_or(&path);
            let expected = baseline.get(original_path).map(|v| v.oid);
            if self.disk_oid(original_path)? != expected {
                bail!("file changed during sync; retry without overwriting: {path}");
            }
            if let Some(version) = baseline.get(original_path) {
                before_modes.insert(path.clone(), version.mode);
            }
            before.insert(path, expected.map(|id| id.to_string()));
        }
        // Target is reachable even if the process stops before the first write.
        self.repo
            .reference("refs/tide/pending", target, true, "Tide pending write")?;
        atomic_json(
            &self.repo.path().join("tide-write.json"),
            &Journal {
                target: target.to_string(),
                before,
                before_modes,
                case_renames,
            },
        )?;
        self.resume_write()
    }

    fn resume_write(&self) -> Result<()> {
        let journal_path = self.repo.path().join("tide-write.json");
        if !journal_path.exists() {
            return Ok(());
        }
        let journal: Journal = serde_json::from_reader(File::open(&journal_path)?)?;
        let target = Oid::from_str(&journal.target)?;
        let files = self.files_at(target)?;
        // Preflight every path before mutating any path.
        for (path, before) in &journal.before {
            let (current, physical_path) = self.journal_disk(path, &journal)?;
            #[cfg(not(unix))]
            let _ = physical_path;
            let desired = files.get(path).map(|v| v.oid);
            let original = before.as_deref().map(Oid::from_str).transpose()?;
            if current != desired && current != original {
                bail!("interrupted write meets newer edit: {path}; run `tide recover <folder>` to preserve current worktree");
            }
            #[cfg(unix)]
            if let (Some(original_mode), Some(desired_version), Some(_)) =
                (journal.before_modes.get(path), files.get(path), current)
            {
                use std::os::unix::fs::PermissionsExt;
                let permissions = fs::metadata(physical_path)?.permissions().mode();
                let mode = if permissions & 0o111 != 0 {
                    0o100755
                } else {
                    0o100644
                };
                if mode != *original_mode && mode != desired_version.mode {
                    bail!("permissions changed during sync: {path}");
                }
            }
        }
        self.install_case_renames(&journal)?;
        for (path, before) in &journal.before {
            let desired = files.get(path).map(|v| v.oid);
            let (current, physical_path) = self.journal_disk(path, &journal)?;
            if current == desired {
                #[cfg(unix)]
                if let Some(version) = files.get(path) {
                    use std::os::unix::fs::PermissionsExt;
                    let full = self.checked_path(path)?;
                    if fs::metadata(&full)?.permissions().mode() & 0o111 != version.mode & 0o111 {
                        fs::set_permissions(
                            full,
                            fs::Permissions::from_mode(version.mode & 0o777),
                        )?;
                    }
                }
                continue;
            }
            if current != before.as_deref().map(Oid::from_str).transpose()? {
                bail!("file changed during write: {path}; retry");
            }
            let full = self.checked_path(path)?;
            if let Some(version) = files.get(path) {
                let parent = full.parent().context("file has no parent")?;
                fs::create_dir_all(parent)?;
                let mut temp = tempfile::NamedTempFile::new_in(parent)?;
                temp.write_all(self.repo.find_blob(version.oid)?.content())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    temp.as_file()
                        .set_permissions(fs::Permissions::from_mode(version.mode & 0o777))?;
                }
                temp.as_file().sync_all()?;
                if self.disk_oid(path)? != current {
                    bail!("file changed before replacement: {path}");
                }
                temp.persist(&full).map_err(|e| e.error)?;
            } else if physical_path.exists() {
                fs::remove_file(physical_path)?;
            }
        }
        self.move_head(target)?;
        fs::remove_file(journal_path)?;
        if let Ok(mut reference) = self.repo.find_reference("refs/tide/pending") {
            reference.delete()?;
        }
        Ok(())
    }

    fn journal_disk(&self, path: &str, journal: &Journal) -> Result<(Option<Oid>, PathBuf)> {
        let current = self.disk_oid(path)?;
        if current.is_none() && !journal.case_renames.is_empty() {
            if let Some(actual) = self.actual_case_path(path)? {
                let relative = actual
                    .strip_prefix(&self.options.path)?
                    .components()
                    .map(|c| c.as_os_str().to_str().context("non-UTF8 path"))
                    .collect::<Result<Vec<_>>>()?
                    .join("/");
                return Ok((self.disk_oid(&relative)?, actual));
            }
        }
        Ok((current, self.checked_path(path)?))
    }

    fn actual_case_path(&self, target: &str) -> Result<Option<PathBuf>> {
        let mut path = self.options.path.clone();
        for component in target.split('/') {
            let names = fs::read_dir(&path)?
                .map(|e| e.map(|e| e.file_name()))
                .collect::<std::io::Result<Vec<_>>>()?;
            let Some(name) = names
                .iter()
                .find(|n| n.as_os_str() == component)
                .or_else(|| {
                    names
                        .iter()
                        .find(|n| n.to_string_lossy().to_lowercase() == component.to_lowercase())
                })
            else {
                return Ok(None);
            };
            path.push(name);
            let relative = path
                .strip_prefix(&self.options.path)?
                .components()
                .map(|c| c.as_os_str().to_str().context("non-UTF8 path"))
                .collect::<Result<Vec<_>>>()?
                .join("/");
            self.checked_path(&relative)?;
        }
        Ok(Some(path))
    }

    fn install_case_renames(&self, journal: &Journal) -> Result<()> {
        // Rename each component, not only the leaf. Each rename is atomic on the
        // same volume; replay recognizes components already at their target name.
        for target in journal.case_renames.values() {
            self.checked_path(target)?;
            let mut parent = self.options.path.clone();
            for component in target.split('/') {
                let names = fs::read_dir(&parent)?
                    .map(|e| e.map(|e| e.file_name()))
                    .collect::<std::io::Result<Vec<_>>>()?;
                if !names.iter().any(|name| name == component) {
                    let old = names
                        .iter()
                        .find(|name| {
                            name.to_string_lossy().to_lowercase() == component.to_lowercase()
                        })
                        .context("case rename source disappeared")?;
                    fs::rename(parent.join(old), parent.join(component))?;
                }
                parent.push(component);
            }
        }
        Ok(())
    }

    pub fn pending_conflicts(&self) -> Result<Vec<Conflict>> {
        let path = self.repo.path().join("tide-conflicts.json");
        if !path.exists() {
            return Ok(vec![]);
        }
        Ok(serde_json::from_reader(File::open(path)?)?)
    }

    pub fn resolve(&mut self, path: &str, local: bool) -> Result<()> {
        let mut pending = self.pending_conflicts()?;
        let conflict = pending
            .iter()
            .find(|c| c.path == path)
            .context("no such pending conflict")?;
        let head = self.snapshot(true)?.0.context("no local history")?;
        let baseline = self.files_at(head)?;
        let candidate = self.repo.refname_to_id("refs/tide/conflict-merge")?;
        let candidate_commit = self.repo.find_commit(candidate)?;
        let original_local = candidate_commit.parent_id(candidate_commit.parent_count() - 1)?;
        let mut files = reconcile(
            &self.repo,
            &self.files_at(original_local)?,
            &baseline,
            &self.files_at(candidate)?,
            ReconcileOptions {
                policy: ConflictPolicy::KeepBoth,
                device: &self.options.device,
                stamp: &Local::now().format("%m-%d %H.%M.%S").to_string(),
                text_extensions: &self.options.text_extensions,
            },
        )?
        .files;
        let oid = Oid::from_str(if local {
            &conflict.local
        } else {
            &conflict.remote
        })?;
        let mode = files.get(path).map(|v| v.mode).unwrap_or(0o100644);
        files.insert(path.into(), Version { oid, mode });
        let parents = if head == candidate {
            vec![head]
        } else {
            vec![candidate, head]
        };
        let id = self.commit(&files, &parents, "Tide: resolve conflict")?;
        self.install(id, &baseline)?;
        self.repo.reference(
            "refs/tide/conflict-merge",
            id,
            true,
            "Tide conflict choices",
        )?;
        pending.retain(|c| c.path != path);
        atomic_json(&self.repo.path().join("tide-conflicts.json"), &pending)?;
        Ok(())
    }

    pub fn diff(&self, revision: &str, path: Option<&str>) -> Result<String> {
        let commit = self.repo.revparse_single(revision)?.peel_to_commit()?;
        let parent = if commit.parent_count() > 0 {
            Some(commit.parent(0)?.tree()?)
        } else {
            None
        };
        let mut options = git2::DiffOptions::new();
        if let Some(path) = path {
            crate::validate_path(path)?;
            options.pathspec(path);
        }
        let diff = self.repo.diff_tree_to_tree(
            parent.as_ref(),
            Some(&commit.tree()?),
            Some(&mut options),
        )?;
        let mut out = Vec::new();
        diff.print(git2::DiffFormat::Patch, |_, _, line| {
            match line.origin() {
                '+' | '-' | ' ' => {
                    out.push(line.origin() as u8);
                    out.extend_from_slice(line.content());
                }
                'F' => out.extend(unquote_header(line.content())),
                _ => out.extend_from_slice(line.content()),
            }
            true
        })?;
        Ok(String::from_utf8_lossy(&out).into_owned())
    }

    pub fn export(&self, revision: &str, path: &str, destination: &Path) -> Result<()> {
        crate::validate_path(path)?;
        let commit = self.repo.revparse_single(revision)?.peel_to_commit()?;
        let files = self.files_at(commit.id())?;
        let version = files
            .get(path)
            .context("file does not exist at this revision")?;
        let mut output = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)?;
        output.write_all(self.repo.find_blob(version.oid)?.content())?;
        output.sync_all()?;
        Ok(())
    }
}

impl GitBackend for LibgitBackend {
    fn sync(&mut self, confirm_initial_merge: bool) -> Result<SyncReport> {
        self.phase(SyncPhase::Collect);
        let pending = self.pending_conflicts()?;
        if !pending.is_empty() {
            return Ok(SyncReport {
                conflicts: pending,
                pending: true,
                ..Default::default()
            });
        }
        let (mut local, warnings) = self.snapshot(true)?;
        let mut report = SyncReport {
            warnings,
            ..Default::default()
        };
        // Folder content before this round; retries re-snapshot after partial writes.
        let before = local
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        for attempt in 0..3 {
            let remote = self.fetch()?;
            let old = local
                .map(|id| self.files_at(id))
                .transpose()?
                .unwrap_or_default();
            let remote_files = remote
                .map(|id| self.files_at(id))
                .transpose()?
                .unwrap_or_default();
            let target = match (local, remote) {
                (None, None) => return Ok(report),
                (Some(l), None) => l,
                (None, Some(r)) => {
                    self.install(r, &old)?;
                    r
                }
                (Some(l), Some(r)) if l == r || self.repo.graph_descendant_of(l, r)? => l,
                (Some(l), Some(r)) if self.repo.graph_descendant_of(r, l)? => {
                    self.install(r, &old)?;
                    r
                }
                (Some(l), Some(r)) => {
                    let base = match self.repo.merge_base(l, r) {
                        Ok(b) => self.files_at(b)?,
                        Err(e) if e.code() == ErrorCode::NotFound => {
                            if !confirm_initial_merge {
                                return Err(SyncFailure::new(FailureKind::NeedsConfirmation, "initial association needs review: local and remote have independent histories; inspect both and rerun with --confirm-initial-merge").into());
                            }
                            Files::new()
                        }
                        Err(e) => return Err(e.into()),
                    };
                    let unrelated = self.repo.merge_base(l, r).is_err();
                    let policy = if unrelated {
                        ConflictPolicy::KeepBoth
                    } else {
                        self.options.policy
                    };
                    self.phase(SyncPhase::Reconcile);
                    let result = reconcile(
                        &self.repo,
                        &base,
                        &old,
                        &remote_files,
                        ReconcileOptions {
                            policy,
                            device: &self.options.device,
                            stamp: &Local::now().format("%m-%d %H.%M.%S").to_string(),
                            text_extensions: &self.options.text_extensions,
                        },
                    )?;
                    if policy == ConflictPolicy::Ask && !result.conflicts.is_empty() {
                        // Hold the round until all choices are durable. Neither side's conflict bytes are discarded.
                        self.repo.reference(
                            "refs/tide/conflict-remote",
                            r,
                            true,
                            "Tide unresolved remote",
                        )?;
                        let candidate =
                            self.commit(&result.files, &[r, l], "Tide: pending conflict choices")?;
                        self.repo.reference(
                            "refs/tide/conflict-merge",
                            candidate,
                            true,
                            "Tide unresolved merge",
                        )?;
                        atomic_json(
                            &self.repo.path().join("tide-conflicts.json"),
                            &result.conflicts,
                        )?;
                        report.conflicts = result.conflicts;
                        report.pending = true;
                        return Ok(report);
                    }
                    report.conflicts.extend(result.conflicts);
                    let id = self.commit(&result.files, &[r, l], "Tide: reconcile devices")?;
                    self.install(id, &old)?;
                    id
                }
            };
            #[cfg(test)]
            if let Some(callback) = &mut self.before_push {
                callback();
            }
            self.phase(SyncPhase::Push);
            match self.push(&self.options.remote, target) {
                Ok(()) => {
                    self.repo
                        .reference("refs/tide/synced", target, true, "Tide synchronized")?;
                    let synced = self.files_at(target)?;
                    report.head = target.to_string();
                    report.changed = count_changes(&before, &synced);
                    report.uploaded = count_changes(&remote_files, &synced);
                    self.phase(SyncPhase::Mirrors);
                    for mirror in &self.options.mirrors {
                        if let Err(e) = self.push(mirror, target) {
                            report.mirror_errors.push(e.to_string());
                        }
                    }
                    self.phase(SyncPhase::Finished);
                    return Ok(report);
                }
                Err(error) if attempt == 2 || !error.downcast_ref::<git2::Error>().is_some_and(|e| e.code() == ErrorCode::NotFastForward) => return Err(error.context(
                    "push failed; local versions are preserved (at most 3 attempts for non-fast-forward rejection)",
                )),
                Err(_) => {
                    local = self.snapshot(true)?.0;
                }
            }
        }
        unreachable!()
    }

    fn history(&self, skip: usize, limit: usize) -> Result<Vec<HistoryEntry>> {
        let Some(head) = self.head()? else {
            return Ok(vec![]);
        };
        let mut walk = self.repo.revwalk()?;
        walk.push_head()?;
        // Local versions replaced by "remote overwrites local" stay restorable from history.
        let mut replaced = BTreeSet::new();
        for reference in self.repo.references_glob("refs/tide/replaced/*")? {
            if let Some(id) = reference?.target() {
                walk.push(id)?;
                replaced.insert(id);
            }
        }
        walk.set_sorting(git2::Sort::TOPOLOGICAL | git2::Sort::TIME)?;
        walk.skip(skip)
            .take(limit.min(50))
            .map(|id| {
                let commit = self.repo.find_commit(id?)?;
                let parent = if commit.parent_count() > 0 {
                    Some(commit.parent(0)?.tree()?)
                } else {
                    None
                };
                let diff =
                    self.repo
                        .diff_tree_to_tree(parent.as_ref(), Some(&commit.tree()?), None)?;
                let device = commit.author().name().unwrap_or("unknown").to_owned();
                let local_only = !replaced.is_empty()
                    && commit.id() != head
                    && !self.repo.graph_descendant_of(head, commit.id())?;
                Ok(HistoryEntry {
                    id: commit.id().to_string(),
                    device,
                    timestamp: commit.time().seconds(),
                    // A replaced ref whose commit is still reachable (older Tide, or an
                    // interrupted replacement) is ordinary history, not a replaced version.
                    message: if local_only && replaced.contains(&commit.id()) {
                        "Tide: replaced local version".into()
                    } else {
                        commit.summary()?.unwrap_or("").into()
                    },
                    changed: diff.deltas().len(),
                    local_only,
                })
            })
            .collect()
    }

    fn restore(&mut self, revision: &str, path: Option<&str>) -> Result<()> {
        if !self.pending_conflicts()?.is_empty() {
            bail!("resolve pending conflicts first");
        }
        let desired = self.repo.revparse_single(revision)?.peel_to_commit()?.id();
        let head = self.snapshot(true)?.0;
        let baseline = head
            .map(|id| self.files_at(id))
            .transpose()?
            .unwrap_or_default();
        let selected = self.files_at(desired)?;
        let files = if let Some(path) = path {
            crate::validate_path(path)?;
            let mut files = baseline.clone();
            if let Some(v) = selected.get(path) {
                files.insert(path.into(), *v);
            } else {
                files.remove(path);
            }
            files
        } else {
            selected
        };
        let id = self.commit(
            &files,
            &head.into_iter().collect::<Vec<_>>(),
            &format!("Tide: restore {desired}"),
        )?;
        self.install(id, &baseline)
    }
}
