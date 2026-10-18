use git2::{Oid, Repository};
use std::{fs, path::Path, time::Duration};
use tide_git::{GitBackend, LibgitBackend, RepoOptions};

fn options(root: &Path, remote: &Path) -> RepoOptions {
    let mut options = RepoOptions::new(
        root.into(),
        remote.to_str().unwrap().into(),
        "device".into(),
    );
    options.stable_for = Duration::ZERO;
    options
}
fn crafted_remote(remote: &Path, entries: &[(&str, &[u8], i32)]) -> Oid {
    let repo = Repository::init_bare(remote).unwrap();
    let mut builder = repo.treebuilder(None).unwrap();
    for (name, bytes, mode) in entries {
        builder
            .insert(name, repo.blob(bytes).unwrap(), *mode)
            .unwrap();
    }
    let tree = repo.find_tree(builder.write().unwrap()).unwrap();
    let signature = git2::Signature::now("remote", "remote@example.com").unwrap();
    repo.commit(
        Some("refs/heads/main"),
        &signature,
        &signature,
        "remote files",
        &tree,
        &[],
    )
    .unwrap()
}

#[test]
fn remote_symlinks_and_case_collisions_are_rejected_before_writing() {
    for symlink in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("docs");
        fs::create_dir(&root).unwrap();
        let remote = temp.path().join("remote.git");
        if symlink {
            crafted_remote(&remote, &[("link", b"../outside", 0o120000)]);
        } else {
            crafted_remote(
                &remote,
                &[("A.txt", b"a", 0o100644), ("a.txt", b"b", 0o100644)],
            );
        }
        let result = LibgitBackend::open(options(&root, &remote))
            .unwrap()
            .sync(false);
        assert!(result.is_err());
        assert!(!root.join("link").exists());
        assert!(!root.join("A.txt").exists());
    }
}

#[test]
fn mirror_failure_does_not_undo_primary_success() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("docs");
    fs::create_dir(&root).unwrap();
    let remote = temp.path().join("remote.git");
    Repository::init_bare(&remote).unwrap();
    fs::write(root.join("note.txt"), b"hello").unwrap();
    let mut options = options(&root, &remote);
    options
        .mirrors
        .push(temp.path().join("missing.git").to_str().unwrap().into());
    let mut backend = LibgitBackend::open(options).unwrap();
    let result = backend.sync(false).unwrap();
    assert_eq!(result.mirror_errors.len(), 1);
    assert_eq!(
        backend
            .repository()
            .refname_to_id("refs/tide/synced")
            .unwrap()
            .to_string(),
        result.head
    );
    assert_eq!(
        Repository::open_bare(&remote)
            .unwrap()
            .refname_to_id("refs/heads/main")
            .unwrap()
            .to_string(),
        result.head
    );
}

#[test]
fn planning_does_not_commit_or_replace_document_bytes() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("docs");
    fs::create_dir(&root).unwrap();
    let remote = temp.path().join("remote.git");
    crafted_remote(&remote, &[("note.txt", b"remote", 0o100644)]);
    fs::write(root.join("note.txt"), b"local").unwrap();
    let backend = LibgitBackend::open(options(&root, &remote)).unwrap();
    let plan = backend.plan().unwrap();
    assert_eq!(
        (
            plan.local_files,
            plan.local_bytes,
            plan.remote_files,
            plan.remote_bytes
        ),
        (1, 5, 1, 6)
    );
    assert!(plan.needs_initial_confirmation);
    assert_eq!(plan.different_at_same_path, vec!["note.txt"]);
    assert!(backend.repository().head().is_err());
    assert_eq!(fs::read(root.join("note.txt")).unwrap(), b"local");
}

#[test]
fn absent_remote_branch_does_not_create_a_typo_branch() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("docs");
    fs::create_dir(&root).unwrap();
    let remote = temp.path().join("remote.git");
    crafted_remote(&remote, &[("remote.txt", b"remote", 0o100644)]);
    fs::write(root.join("local.txt"), b"local").unwrap();
    let mut options = options(&root, &remote);
    options.branch = "typo".into();
    let mut backend = LibgitBackend::open(options).unwrap();
    assert!(backend.sync(false).is_err());
    assert!(Repository::open_bare(&remote)
        .unwrap()
        .refname_to_id("refs/heads/typo")
        .is_err());
    assert_eq!(fs::read(root.join("local.txt")).unwrap(), b"local");
}

#[test]
fn choosing_local_for_multiple_ask_conflicts_retains_remote_nonconflicting_files() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    let remote = temp.path().join("remote.git");
    fs::create_dir(&a).unwrap();
    fs::create_dir(&b).unwrap();
    Repository::init_bare(&remote).unwrap();
    for name in ["one.bin", "two.bin"] {
        fs::write(a.join(name), b"base").unwrap();
    }
    LibgitBackend::open(options(&a, &remote))
        .unwrap()
        .sync(false)
        .unwrap();
    LibgitBackend::open(options(&b, &remote))
        .unwrap()
        .sync(false)
        .unwrap();
    for name in ["one.bin", "two.bin"] {
        fs::write(a.join(name), b"remote").unwrap();
        fs::write(b.join(name), b"local").unwrap();
    }
    fs::write(a.join("new.txt"), b"remote addition").unwrap();
    LibgitBackend::open(options(&a, &remote))
        .unwrap()
        .sync(false)
        .unwrap();
    let mut opt = options(&b, &remote);
    opt.policy = tide_git::ConflictPolicy::Ask;
    let mut backend = LibgitBackend::open(opt).unwrap();
    assert!(backend.sync(false).unwrap().pending);
    backend.resolve("one.bin", true).unwrap();
    assert!(backend.sync(false).unwrap().pending);
    backend.resolve("two.bin", true).unwrap();
    assert!(!backend.sync(false).unwrap().pending);
    for name in ["one.bin", "two.bin"] {
        assert_eq!(fs::read(b.join(name)).unwrap(), b"local");
    }
    assert_eq!(fs::read(b.join("new.txt")).unwrap(), b"remote addition");
}
