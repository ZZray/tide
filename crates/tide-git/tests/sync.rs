use git2::{Oid, Repository};
use std::{
    fs,
    path::{Path, PathBuf},
    time::Duration,
};
use tide_git::{ConflictPolicy, GitBackend, LibgitBackend, RepoOptions};

struct Pair {
    _temp: tempfile::TempDir,
    remote: PathBuf,
    a: PathBuf,
    b: PathBuf,
}
impl Pair {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let remote = temp.path().join("remote.git");
        Repository::init_bare(&remote).unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        Self {
            _temp: temp,
            remote,
            a,
            b,
        }
    }
    fn options(&self, path: &Path, device: &str, policy: ConflictPolicy) -> RepoOptions {
        let mut o = RepoOptions::new(
            path.into(),
            self.remote.to_str().unwrap().into(),
            device.into(),
        );
        o.stable_for = Duration::ZERO;
        o.policy = policy;
        o
    }
    fn open(&self, path: &Path, device: &str, policy: ConflictPolicy) -> LibgitBackend {
        LibgitBackend::open(self.options(path, device, policy)).unwrap()
    }
    fn sync(&self, path: &Path) -> tide_git::SyncReport {
        self.open(path, "device", ConflictPolicy::KeepBoth)
            .sync(false)
            .unwrap()
    }
    fn seed(&self, path: &str, bytes: &[u8]) -> Oid {
        put(&self.a, path, bytes);
        let report = self.sync(&self.a);
        self.sync(&self.b);
        Oid::from_str(&report.head).unwrap()
    }
    fn remote_head(&self) -> Oid {
        Repository::open_bare(&self.remote)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .unwrap()
    }
}
fn put(root: &Path, path: &str, data: &[u8]) {
    let p = root.join(path);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}
fn read(root: &Path, path: &str) -> Vec<u8> {
    fs::read(root.join(path)).unwrap()
}
fn copies(root: &Path) -> Vec<PathBuf> {
    fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.file_name().unwrap().to_string_lossy().contains("冲突"))
        .collect()
}

#[test]
fn uploads_downloads_and_preserves_exact_bytes() {
    let p = Pair::new();
    let bytes = b"hello\r\nworld\r\n\0\xff";
    p.seed("中文/文档.bin", bytes);
    assert_eq!(read(&p.b, "中文/文档.bin"), bytes);
    put(&p.b, "new.txt", b"new\n");
    p.sync(&p.b);
    p.sync(&p.a);
    assert_eq!(read(&p.a, "new.txt"), b"new\n");
    fs::remove_file(p.a.join("new.txt")).unwrap();
    p.sync(&p.a);
    p.sync(&p.b);
    assert!(!p.b.join("new.txt").exists());
}

#[test]
fn nonoverlapping_text_edits_merge_without_markers() {
    let p = Pair::new();
    p.seed("note.md", b"one\ntwo\nthree\nfour\nfive\n");
    put(&p.a, "note.md", b"ONE\ntwo\nthree\nfour\nfive\n");
    put(&p.b, "note.md", b"one\ntwo\nthree\nfour\nFIVE\n");
    p.sync(&p.a);
    let report = p.sync(&p.b);
    p.sync(&p.a);
    assert!(report.conflicts.is_empty());
    assert_eq!(read(&p.a, "note.md"), b"ONE\ntwo\nthree\nfour\nFIVE\n");
}

#[test]
fn reconciliation_table_and_policies() {
    for policy in [
        ConflictPolicy::KeepBoth,
        ConflictPolicy::PreferLocal,
        ConflictPolicy::PreferRemote,
        ConflictPolicy::Ask,
    ] {
        let p = Pair::new();
        let baseline = p.seed("file.bin", b"base\0");
        put(&p.a, "file.bin", b"remote\0");
        put(&p.b, "file.bin", b"local\0");
        p.sync(&p.a);
        let mut b = p.open(&p.b, "笔记本:/bad", policy);
        let report = b.sync(false).unwrap();
        assert_eq!(report.conflicts.len(), 1);
        if policy == ConflictPolicy::Ask {
            assert!(report.pending);
            assert_eq!(read(&p.b, "file.bin"), b"local\0");
            drop(b);
            let mut b = p.open(&p.b, "b", policy);
            assert_eq!(b.pending_conflicts().unwrap().len(), 1);
            b.resolve("file.bin", false).unwrap();
            assert!(b.pending_conflicts().unwrap().is_empty());
            b.sync(false).unwrap();
            assert_eq!(read(&p.b, "file.bin"), b"remote\0");
        } else {
            let expected: &[u8] = if policy == ConflictPolicy::PreferLocal {
                b"local\0"
            } else {
                b"remote\0"
            };
            assert_eq!(read(&p.b, "file.bin"), expected);
            if policy == ConflictPolicy::KeepBoth {
                let copies = copies(&p.b);
                assert_eq!(copies.len(), 1);
                assert_eq!(fs::read(&copies[0]).unwrap(), b"local\0");
            }
            let head = b.repository().head().unwrap().target().unwrap();
            assert!(b.repository().graph_descendant_of(head, baseline).unwrap());
        }
    }
}

#[test]
fn deletion_vs_modification_keeps_modified_file_on_either_side() {
    for delete_local in [false, true] {
        let p = Pair::new();
        p.seed("file.txt", b"base");
        let (delete, modify) = if delete_local {
            (&p.b, &p.a)
        } else {
            (&p.a, &p.b)
        };
        fs::remove_file(delete.join("file.txt")).unwrap();
        put(modify, "file.txt", b"modified");
        p.sync(&p.a);
        p.sync(&p.b);
        p.sync(&p.a);
        assert_eq!(read(&p.a, "file.txt"), b"modified");
        assert_eq!(read(&p.b, "file.txt"), b"modified");
    }
}

#[test]
fn same_change_is_not_a_conflict_and_simultaneous_add_keeps_both() {
    let p = Pair::new();
    p.seed("base.md", b"base");
    put(&p.a, "base.md", b"same");
    put(&p.b, "base.md", b"same");
    put(&p.a, "new.bin", b"a");
    put(&p.b, "new.bin", b"b");
    p.sync(&p.a);
    let report = p.sync(&p.b);
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(report.conflicts[0].path, "new.bin");
    assert_eq!(read(&p.b, "base.md"), b"same");
}

#[test]
fn independent_histories_require_confirmation_and_keep_both() {
    let p = Pair::new();
    put(&p.a, "note.md", b"remote");
    p.sync(&p.a);
    put(&p.b, "note.md", b"local");
    let mut b = p.open(&p.b, "b", ConflictPolicy::PreferRemote);
    assert!(b
        .sync(false)
        .unwrap_err()
        .to_string()
        .contains("initial association"));
    assert_eq!(read(&p.b, "note.md"), b"local");
    let report = b.sync(true).unwrap();
    assert_eq!(report.conflicts.len(), 1);
    assert_eq!(copies(&p.b).len(), 1);
}

#[test]
fn guard_stops_mass_deletions_without_committing_or_pushing() {
    let p = Pair::new();
    for n in 0..100 {
        put(&p.a, &format!("{n}.txt"), b"keep");
    }
    let before = p.sync(&p.a).head;
    for n in 0..51 {
        fs::remove_file(p.a.join(format!("{n}.txt"))).unwrap();
    }
    let mut a = p.open(&p.a, "a", ConflictPolicy::KeepBoth);
    assert!(a
        .sync(false)
        .unwrap_err()
        .to_string()
        .contains("deletion guard"));
    assert_eq!(p.remote_head().to_string(), before);
    assert_eq!(
        a.repository().head().unwrap().target().unwrap().to_string(),
        before
    );
}

#[test]
fn missing_root_cannot_delete_remote_and_lock_excludes_second_worker() {
    let p = Pair::new();
    let before = p.seed("keep.txt", b"keep");
    let mut a = p.open(&p.a, "a", ConflictPolicy::KeepBoth);
    assert!(LibgitBackend::open(p.options(&p.a, "other", ConflictPolicy::KeepBoth)).is_err());
    // Windows locks the repo's directory while libgit2 is alive. Check missing-root handling at open.
    assert!(LibgitBackend::open(RepoOptions::new(
        p.a.join("missing"),
        p.remote.to_str().unwrap().into(),
        "x".into()
    ))
    .is_err());
    a.sync(false).unwrap();
    assert_eq!(p.remote_head(), before);
}

#[test]
fn restore_and_overwrite_are_new_commits_and_old_versions_survive() {
    let p = Pair::new();
    let original = p.seed("file.txt", b"v1");
    put(&p.a, "file.txt", b"v2");
    let second = Oid::from_str(&p.sync(&p.a).head).unwrap();
    let mut a = p.open(&p.a, "a", ConflictPolicy::KeepBoth);
    a.restore(&original.to_string(), Some("file.txt")).unwrap();
    assert_eq!(read(&p.a, "file.txt"), b"v1");
    let head = a.repository().head().unwrap().target().unwrap();
    assert!(a.repository().graph_descendant_of(head, second).unwrap());
    a.sync(false).unwrap();
    drop(a);
    put(&p.a, "file.txt", b"unpublished");
    let mut a = p.open(&p.a, "a", ConflictPolicy::KeepBoth);
    let report = a.overwrite(false, false).unwrap();
    assert_eq!(report.changed, 1);
    assert_eq!(read(&p.a, "file.txt"), b"v1");
    assert!(a
        .repository()
        .references_glob("refs/tide/replaced/*")
        .unwrap()
        .next()
        .is_some());
    // The replaced version is listed locally and restorable, but never published.
    let replaced = a
        .history(0, 50)
        .unwrap()
        .into_iter()
        .find(|e| e.message == "Tide: replaced local version")
        .unwrap();
    assert!(replaced.local_only);
    assert!(a
        .history(0, 50)
        .unwrap()
        .iter()
        .filter(|e| !e.local_only)
        .all(|e| e.message != "Tide: replaced local version"));
    a.sync(false).unwrap();
    let unpublished = Oid::hash_object(git2::ObjectType::Blob, b"unpublished").unwrap();
    assert!(!Repository::open_bare(&p.remote)
        .unwrap()
        .odb()
        .unwrap()
        .exists(unpublished));
    a.restore(&replaced.id, Some("file.txt")).unwrap();
    assert_eq!(read(&p.a, "file.txt"), b"unpublished");
    put(&p.a, "file.txt", b"replace remote");
    a.overwrite(true, false).unwrap();
    drop(a);
    p.sync(&p.b);
    assert_eq!(read(&p.b, "file.txt"), b"replace remote");
}

#[test]
fn unstable_large_staged_and_index_lock_do_not_push() {
    let p = Pair::new();
    let before = p.seed("file.txt", b"v1");
    put(&p.a, "file.txt", b"v2");
    let mut options = p.options(&p.a, "a", ConflictPolicy::KeepBoth);
    options.stable_for = Duration::from_secs(3600);
    assert!(LibgitBackend::open(options)
        .unwrap()
        .sync(false)
        .unwrap_err()
        .to_string()
        .contains("still being written"));
    let mut options = p.options(&p.a, "a", ConflictPolicy::KeepBoth);
    options.reject_bytes = 1;
    assert!(LibgitBackend::open(options)
        .unwrap()
        .sync(false)
        .unwrap_err()
        .to_string()
        .contains("size limit"));
    let repo = Repository::open(&p.a).unwrap();
    let mut index = repo.index().unwrap();
    index.add_path(Path::new("file.txt")).unwrap();
    index.write().unwrap();
    drop(index);
    assert!(p
        .open(&p.a, "a", ConflictPolicy::KeepBoth)
        .sync(false)
        .unwrap_err()
        .to_string()
        .contains("staged"));
    fs::write(repo.path().join("index.lock"), b"").unwrap();
    assert!(LibgitBackend::open(p.options(&p.a, "a", ConflictPolicy::KeepBoth)).is_err());
    assert_eq!(p.remote_head(), before);
}

#[test]
fn ignore_rules_and_external_history_directory() {
    let p = Pair::new();
    let mut options = p.options(&p.a, "a", ConflictPolicy::KeepBoth);
    options.git_dir = Some(p._temp.path().join("history.git"));
    options.ignore = vec!["*.tmp".into()];
    put(&p.a, "note.txt", b"hello");
    put(&p.a, "skip.tmp", b"ignored");
    let mut a = LibgitBackend::open(options.clone()).unwrap();
    a.sync(false).unwrap();
    drop(a);
    assert!(!p.a.join(".git").exists());
    LibgitBackend::open(options).unwrap().sync(false).unwrap();
    p.sync(&p.b);
    assert_eq!(read(&p.b, "note.txt"), b"hello");
    assert!(!p.b.join("skip.tmp").exists());
}

#[test]
fn history_pages_diff_and_export() {
    let p = Pair::new();
    let first = p.seed("note.txt", b"v1\n");
    put(&p.a, "note.txt", b"v2\n");
    p.sync(&p.a);
    let a = p.open(&p.a, "a", ConflictPolicy::KeepBoth);
    assert_eq!(a.history(0, 50).unwrap().len(), 2);
    assert_eq!(a.history(1, 50).unwrap().len(), 1);
    let diff = a.diff("HEAD", Some("note.txt")).unwrap();
    assert!(diff.contains("-v1"));
    assert!(diff.contains("+v2"));
    let output = p._temp.path().join("export.txt");
    a.export(&first.to_string(), "note.txt", &output).unwrap();
    assert_eq!(fs::read(&output).unwrap(), b"v1\n");
    assert!(a.export(&first.to_string(), "note.txt", &output).is_err());
}

#[test]
fn reports_count_local_and_remote_changes_in_every_path() {
    let p = Pair::new();
    put(&p.a, "one.txt", b"1");
    put(&p.a, "two.txt", b"2");
    let first = p.sync(&p.a);
    assert_eq!((first.changed, first.uploaded), (0, 2));
    // Fresh download into an empty folder.
    let download = p.sync(&p.b);
    assert_eq!((download.changed, download.uploaded), (2, 0));
    // Fast-forward: A edits one, adds one, deletes one.
    put(&p.a, "one.txt", b"1b");
    put(&p.a, "three.txt", b"3");
    fs::remove_file(p.a.join("two.txt")).unwrap();
    let upload = p.sync(&p.a);
    assert_eq!((upload.changed, upload.uploaded), (0, 3));
    let pull = p.sync(&p.b);
    assert_eq!((pull.changed, pull.uploaded), (3, 0));
    // Reconcile: both sides changed different files.
    put(&p.a, "one.txt", b"1c");
    put(&p.b, "three.txt", b"3b");
    p.sync(&p.a);
    let merged = p.sync(&p.b);
    assert_eq!((merged.changed, merged.uploaded), (1, 1));
    let idle = p.sync(&p.b);
    assert_eq!((idle.changed, idle.uploaded), (0, 0));
}

#[test]
fn diff_headers_show_unicode_paths() {
    let p = Pair::new();
    p.seed("书单/第一册.md", b"v1\n");
    put(&p.a, "书单/第一册.md", b"v2\n");
    p.sync(&p.a);
    let a = p.open(&p.a, "a", ConflictPolicy::KeepBoth);
    let diff = a.diff("HEAD", None).unwrap();
    assert!(diff.contains("a/书单/第一册.md"), "{diff}");
    assert!(diff.contains("+++ b/书单/第一册.md"), "{diff}");
    assert!(!diff.contains("\\344"), "{diff}");
    assert!(diff.contains("+v2"));
}

#[test]
fn replacing_local_from_an_empty_remote_is_refused() {
    let p = Pair::new();
    put(&p.a, "keep.txt", b"keep");
    let mut a = p.open(&p.a, "a", ConflictPolicy::KeepBoth);
    let error = a.overwrite(false, false).unwrap_err().to_string();
    assert!(error.contains("no content"), "{error}");
    assert_eq!(read(&p.a, "keep.txt"), b"keep");
}

#[test]
fn thousand_alternating_rounds_preserve_versions() {
    let p = Pair::new();
    p.seed("counter.txt", b"0\n");
    let mut previous = p.remote_head();
    for n in 1..=1000 {
        let (source, other) = if n % 2 == 0 {
            (&p.a, &p.b)
        } else {
            (&p.b, &p.a)
        };
        let bytes = format!("{n}\n");
        put(source, "counter.txt", bytes.as_bytes());
        p.sync(source);
        p.sync(other);
        assert_eq!(read(source, "counter.txt"), bytes.as_bytes());
        assert_eq!(read(other, "counter.txt"), bytes.as_bytes());
        let remote = Repository::open_bare(&p.remote).unwrap();
        let next = p.remote_head();
        assert!(remote.graph_descendant_of(next, previous).unwrap());
        previous = next;
    }
}
