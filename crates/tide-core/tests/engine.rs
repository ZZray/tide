use std::{fs, path::PathBuf, thread, time::Duration};
use tide_core::{Config, Folder};

fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let config = temp.path().join("config.toml");
    let folder = temp.path().join("documents");
    fs::create_dir(&folder).unwrap();
    let remote = temp.path().join("remote.git");
    git2::Repository::init_bare(&remote).unwrap();
    Config::init(&config).unwrap();
    Config::edit(&config, |c| {
        c.folders.push(Folder::new(
            "notes".into(),
            folder.clone(),
            remote.to_str().unwrap().into(),
        ));
        Ok(())
    })
    .unwrap();
    (temp, config, folder)
}

#[test]
fn results_are_durable_and_missing_root_persists_pause() {
    let (_temp, config, folder) = fixture();
    fs::write(folder.join("note.txt"), b"hello").unwrap();
    thread::sleep(Duration::from_millis(2100));
    let run = tide_core::sync_folder(&config, "notes", false).unwrap();
    assert!(run.success, "{run:?}");
    let records = tide_core::runs(&config).unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].run_id, run.run_id);
    fs::rename(&folder, folder.with_file_name("unplugged")).unwrap();
    let run = tide_core::sync_folder(&config, "notes", false).unwrap();
    assert!(!run.success);
    assert!(Config::load(&config).unwrap().folders[0].paused);
    assert_eq!(tide_core::runs(&config).unwrap().len(), 2);
}

#[test]
fn mass_delete_pauses_without_changing_remote() {
    let (_temp, config, folder) = fixture();
    for n in 0..60 {
        fs::write(folder.join(format!("{n}.txt")), b"keep").unwrap();
    }
    thread::sleep(Duration::from_millis(2100));
    let run = tide_core::sync_folder(&config, "notes", false).unwrap();
    assert!(run.success);
    for n in 0..51 {
        fs::remove_file(folder.join(format!("{n}.txt"))).unwrap();
    }
    let run = tide_core::sync_folder(&config, "notes", false).unwrap();
    assert!(!run.success);
    assert!(run.error.unwrap().contains("remote is unchanged"));
    assert!(Config::load(&config).unwrap().folders[0].paused);
}
