use super::*;

#[test]
fn filtered_clean_worktree_is_snapshotted_as_raw_bytes() {
    let f = Fixture::new();
    f.seed();
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    let head = a.head().unwrap().unwrap();
    let baseline = a.files_at(head).unwrap();
    let mut files = baseline.clone();
    files.insert(
        ".gitattributes".into(),
        Version {
            oid: a.repo.blob(b"*.md text eol=crlf\n").unwrap(),
            mode: 0o100644,
        },
    );
    files.insert(
        "note.md".into(),
        Version {
            oid: a.repo.blob(b"line\n").unwrap(),
            mode: 0o100644,
        },
    );
    let target = a.commit(&files, &[head], "filtered file").unwrap();
    a.install(target, &baseline).unwrap();
    fs::write(f.a.join("note.md"), b"line\r\n").unwrap();
    let mut index = a.repo.index().unwrap();
    index.add_path(Path::new("note.md")).unwrap();
    index.write().unwrap();
    drop(index);
    assert_eq!(
        a.repo.status_file(Path::new("note.md")).unwrap(),
        Status::CURRENT
    );
    let snapshot = a.snapshot(true).unwrap().0.unwrap();
    let actual = a.files_at(snapshot).unwrap();
    assert_eq!(
        a.repo.find_blob(actual["note.md"].oid).unwrap().content(),
        b"line\r\n"
    );
    assert_eq!(fs::read(f.a.join("note.md")).unwrap(), b"line\r\n");
    fs::write(f.a.join("note.md"), b"line\r\n").unwrap();
    assert!(a.changes().unwrap().is_empty());
    assert_eq!(a.snapshot(true).unwrap().0, Some(snapshot));
}

#[test]
fn case_only_remote_rename_keeps_bytes_and_converges() {
    let f = Fixture::new();
    f.seed();
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    let head = a.head().unwrap().unwrap();
    let baseline = a.files_at(head).unwrap();
    let mut files = baseline.clone();
    let version = files.remove("one.txt").unwrap();
    files.insert("One.txt".into(), version);
    let target = a.commit(&files, &[head], "case rename").unwrap();
    a.install(target, &baseline).unwrap();
    assert_eq!(fs::read(f.a.join("One.txt")).unwrap(), b"base1");
    let names = fs::read_dir(&f.a)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(names.iter().any(|name| name == "One.txt"));
    assert!(!names.iter().any(|name| name == "one.txt"));
    assert!(a.changes().unwrap().is_empty());
}

#[test]
fn directory_case_rename_replays_after_partial_completion() {
    let f = Fixture::new();
    f.seed();
    fs::create_dir_all(f.a.join("docs/sub")).unwrap();
    fs::write(f.a.join("docs/sub/note.txt"), b"nested").unwrap();
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    let head = a.snapshot(true).unwrap().0.unwrap();
    let baseline = a.files_at(head).unwrap();
    let mut files = baseline.clone();
    let v = files.remove("docs/sub/note.txt").unwrap();
    files.insert("Docs/Sub/Note.txt".into(), v);
    let target = a.commit(&files, &[head], "directory case rename").unwrap();
    let journal = Journal {
        target: target.to_string(),
        before: BTreeMap::from([("Docs/Sub/Note.txt".into(), Some(v.oid.to_string()))]),
        before_modes: BTreeMap::from([("Docs/Sub/Note.txt".into(), v.mode)]),
        case_renames: BTreeMap::from([("docs/sub/note.txt".into(), "Docs/Sub/Note.txt".into())]),
    };
    a.repo
        .reference("refs/tide/pending", target, true, "test")
        .unwrap();
    atomic_json(&a.repo.path().join("tide-write.json"), &journal).unwrap();
    fs::rename(f.a.join("docs"), f.a.join("Docs")).unwrap();
    drop(a);
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    assert_eq!(fs::read(f.a.join("Docs/Sub/Note.txt")).unwrap(), b"nested");
    for (parent, expected) in [
        (&f.a, "Docs"),
        (&f.a.join("Docs"), "Sub"),
        (&f.a.join("Docs/Sub"), "Note.txt"),
    ] {
        assert!(fs::read_dir(parent)
            .unwrap()
            .any(|e| e.unwrap().file_name() == expected));
    }
    assert!(a.changes().unwrap().is_empty());
}

#[test]
fn directory_case_rename_with_sibling_deletion_does_not_resurrect_file() {
    let f = Fixture::new();
    f.seed();
    fs::create_dir_all(f.a.join("docs")).unwrap();
    fs::write(f.a.join("docs/a.txt"), b"a").unwrap();
    fs::write(f.a.join("docs/b.txt"), b"b").unwrap();
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    let head = a.snapshot(true).unwrap().0.unwrap();
    let baseline = a.files_at(head).unwrap();
    let mut files = baseline.clone();
    let v = files.remove("docs/a.txt").unwrap();
    files.remove("docs/b.txt");
    files.insert("Docs/a.txt".into(), v);
    let target = a.commit(&files, &[head], "rename and delete").unwrap();
    a.install(target, &baseline).unwrap();
    assert_eq!(fs::read(f.a.join("Docs/a.txt")).unwrap(), b"a");
    assert!(!f.a.join("Docs/b.txt").exists());
    assert!(!f.a.join("docs/b.txt").exists());
    assert!(a.changes().unwrap().is_empty());
}

#[cfg(unix)]
#[test]
fn mode_only_remote_change_is_installed_without_reversing_it() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    f.seed();
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    let head = a.head().unwrap().unwrap();
    let baseline = a.files_at(head).unwrap();
    let mut files = baseline.clone();
    files.get_mut("one.txt").unwrap().mode = 0o100755;
    let target = a.commit(&files, &[head], "chmod").unwrap();
    a.install(target, &baseline).unwrap();
    assert_ne!(
        fs::metadata(f.a.join("one.txt"))
            .unwrap()
            .permissions()
            .mode()
            & 0o111,
        0
    );
    assert!(a.changes().unwrap().is_empty());
}

struct Fixture {
    _temp: tempfile::TempDir,
    a: PathBuf,
    b: PathBuf,
    remote: PathBuf,
}
impl Fixture {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let a = temp.path().join("a");
        let b = temp.path().join("b");
        fs::create_dir(&a).unwrap();
        fs::create_dir(&b).unwrap();
        let remote = temp.path().join("remote.git");
        Repository::init_bare(&remote).unwrap();
        Self {
            _temp: temp,
            a,
            b,
            remote,
        }
    }
    fn options(&self, path: &Path) -> RepoOptions {
        let mut o = RepoOptions::new(
            path.into(),
            self.remote.to_str().unwrap().into(),
            "test".into(),
        );
        o.stable_for = Duration::ZERO;
        o
    }
    fn seed(&self) {
        fs::write(self.a.join("one.txt"), b"base1").unwrap();
        fs::write(self.a.join("two.txt"), b"base2").unwrap();
        LibgitBackend::open(self.options(&self.a))
            .unwrap()
            .sync(false)
            .unwrap();
        LibgitBackend::open(self.options(&self.b))
            .unwrap()
            .sync(false)
            .unwrap();
    }
}

#[test]
fn a_remote_update_between_fetch_and_push_is_reconciled() {
    let f = Fixture::new();
    f.seed();
    fs::write(f.a.join("one.txt"), b"from A").unwrap();
    let b_path = f.b.clone();
    let b_options = f.options(&f.b);
    let mut once = false;
    let mut a = LibgitBackend::open(f.options(&f.a)).unwrap();
    a.before_push = Some(Box::new(move || {
        if !once {
            once = true;
            fs::write(b_path.join("two.txt"), b"from B").unwrap();
            LibgitBackend::open(b_options.clone())
                .unwrap()
                .sync(false)
                .unwrap();
        }
    }));
    let report = a.sync(false).unwrap();
    assert!(!report.head.is_empty());
    assert_eq!(fs::read(f.a.join("one.txt")).unwrap(), b"from A");
    assert_eq!(fs::read(f.a.join("two.txt")).unwrap(), b"from B");
    let remote = Repository::open_bare(&f.remote).unwrap();
    assert_eq!(
        remote.refname_to_id("refs/heads/main").unwrap().to_string(),
        report.head
    );
}

#[test]
fn three_push_rejections_leave_synced_pointer_unchanged() {
    let f = Fixture::new();
    f.seed();
    fs::write(f.a.join("one.txt"), b"from A").unwrap();
    let b_path = f.b.clone();
    let b_options = f.options(&f.b);
    let mut n = 0;
    let mut a = LibgitBackend::open(f.options(&f.a)).unwrap();
    let synced = a.repo.refname_to_id("refs/tide/synced").unwrap();
    a.before_push = Some(Box::new(move || {
        n += 1;
        fs::write(b_path.join("two.txt"), format!("B{n}")).unwrap();
        LibgitBackend::open(b_options.clone())
            .unwrap()
            .sync(false)
            .unwrap();
    }));
    assert!(a.sync(false).is_err());
    assert_eq!(a.repo.refname_to_id("refs/tide/synced").unwrap(), synced);
    assert_eq!(fs::read(f.a.join("one.txt")).unwrap(), b"from A");
}

fn interrupted(f: &Fixture) -> Oid {
    f.seed();
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    let head = a.head().unwrap().unwrap();
    let mut files = a.files_at(head).unwrap();
    files.get_mut("one.txt").unwrap().oid = a.repo.blob(b"remote1").unwrap();
    files.get_mut("two.txt").unwrap().oid = a.repo.blob(b"remote2").unwrap();
    let target = a.commit(&files, &[head], "interrupted target").unwrap();
    let before = a
        .files_at(head)
        .unwrap()
        .into_iter()
        .map(|(p, v)| (p, Some(v.oid.to_string())))
        .collect();
    a.repo
        .reference("refs/tide/pending", target, true, "test")
        .unwrap();
    atomic_json(
        &a.repo.path().join("tide-write.json"),
        &Journal {
            target: target.to_string(),
            before,
            before_modes: BTreeMap::new(),
            case_renames: BTreeMap::new(),
        },
    )
    .unwrap();
    fs::write(f.a.join("one.txt"), b"remote1").unwrap();
    target
}

#[test]
fn interrupted_partial_write_resumes_idempotently() {
    let f = Fixture::new();
    let target = interrupted(&f);
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    assert_eq!(a.head().unwrap(), Some(target));
    assert_eq!(fs::read(f.a.join("two.txt")).unwrap(), b"remote2");
    assert!(!a.repo.path().join("tide-write.json").exists());
}

#[test]
fn interrupted_write_never_overwrites_a_newer_edit_and_can_be_recovered() {
    let f = Fixture::new();
    let target = interrupted(&f);
    fs::write(f.a.join("two.txt"), b"user edit").unwrap();
    assert!(LibgitBackend::open(f.options(&f.a)).is_err());
    assert_eq!(fs::read(f.a.join("two.txt")).unwrap(), b"user edit");
    let mut a = LibgitBackend::open_for_recovery(f.options(&f.a)).unwrap();
    a.recover_keep_worktree().unwrap();
    assert_eq!(fs::read(f.a.join("two.txt")).unwrap(), b"user edit");
    assert!(a
        .repo
        .graph_descendant_of(a.head().unwrap().unwrap(), target)
        .unwrap());
    drop(a);
    LibgitBackend::open(f.options(&f.a)).unwrap();
}

#[test]
fn edit_during_fetch_cannot_be_overwritten_on_install() {
    let f = Fixture::new();
    f.seed();
    let a = LibgitBackend::open(f.options(&f.a)).unwrap();
    let head = a.head().unwrap().unwrap();
    let baseline = a.files_at(head).unwrap();
    let mut files = baseline.clone();
    files.get_mut("one.txt").unwrap().oid = a.repo.blob(b"remote").unwrap();
    let target = a.commit(&files, &[head], "remote change").unwrap();
    fs::write(f.a.join("one.txt"), b"new local edit").unwrap();
    assert!(a.install(target, &baseline).is_err());
    assert_eq!(fs::read(f.a.join("one.txt")).unwrap(), b"new local edit");
    assert_eq!(a.head().unwrap(), Some(head));
}
