use std::{
    fs,
    sync::mpsc::Receiver,
    thread,
    time::{Duration, Instant},
};
use tide_core::{Config, Folder, Payload, Service, ServiceEvent, Task};

fn wait(
    events: &Receiver<ServiceEvent>,
    predicate: impl Fn(&ServiceEvent) -> bool,
) -> ServiceEvent {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let event = events
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("service event timed out");
        if predicate(&event) {
            return event;
        }
    }
}
fn stop(service: Service, events: Receiver<ServiceEvent>) {
    service.stop();
    while events.recv_timeout(Duration::from_secs(5)).is_ok() {}
}

#[test]
fn configuration_hot_reload_and_per_folder_queue_use_real_git() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    Config::init(&path).unwrap();
    let (service, events) = Service::start(path.clone()).unwrap();
    wait(&events, |e| matches!(e, ServiceEvent::Config(_)));
    assert!(Service::start(path.clone()).is_err());
    let local = temp.path().join("docs");
    fs::create_dir(&local).unwrap();
    let remote = temp.path().join("remote.git");
    git2::Repository::init_bare(&remote).unwrap();
    let mut folder = Folder::new(
        "notes".into(),
        local.clone(),
        remote.to_str().unwrap().into(),
    );
    folder.interval = "0s".into();
    folder.on_change = false;
    let id = folder.id.clone();
    Config::edit(&path, |config| {
        config.folders.push(folder);
        Ok(())
    })
    .unwrap();
    service.reload();
    wait(
        &events,
        |e| matches!(e,ServiceEvent::Config(c) if c.folders.len()==1),
    );
    fs::write(local.join("note.md"), b"hello").unwrap();
    thread::sleep(Duration::from_millis(2100));
    service.run(
        &id,
        1,
        Task::Sync {
            confirm_initial: false,
        },
    );
    service.run(&id, 2, Task::History { skip: 0 });
    let mut active = 0;
    let mut completed = 0;
    while completed < 2 {
        match events.recv_timeout(Duration::from_secs(15)).unwrap() {
            ServiceEvent::Started { .. } => {
                active += 1;
                assert_eq!(active, 1, "same folder ran concurrently");
            }
            ServiceEvent::Completed { payload, .. } => {
                active -= 1;
                completed += 1;
                if let Payload::Sync(run) = payload {
                    assert!(run.success, "{run:?}");
                }
            }
            ServiceEvent::Failed { error, .. } => panic!("{error}"),
            _ => (),
        }
    }
    assert!(git2::Repository::open_bare(&remote)
        .unwrap()
        .refname_to_id("refs/heads/main")
        .is_ok());
    Config::edit(&path, |config| {
        config.folders[0].paused = true;
        Ok(())
    })
    .unwrap();
    service.reload();
    wait(
        &events,
        |e| matches!(e,ServiceEvent::Config(c) if c.folders[0].paused),
    );
    service.run(
        &id,
        3,
        Task::Sync {
            confirm_initial: false,
        },
    );
    wait(&events, |e| {
        matches!(e, ServiceEvent::Failed { request: 3, .. })
    });
    service.run(&id, 4, Task::History { skip: 0 });
    wait(&events, |e| {
        matches!(e, ServiceEvent::Started { request: 4, .. })
    });
    service.shutdown().unwrap();
    assert!(events
        .try_iter()
        .any(|e| matches!(e, ServiceEvent::Completed { request: 4, .. })));
    // shutdown must release the watcher lock synchronously, before returning.
    let (reopened, reopened_events) = Service::start(path).unwrap();
    stop(reopened, reopened_events);
}

#[test]
fn invalid_reload_keeps_last_valid_config_and_unknown_task_finishes() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("config.toml");
    Config::init(&path).unwrap();
    let local = temp.path().join("docs");
    fs::create_dir(&local).unwrap();
    let mut folder = Folder::new("retained".into(), local, "unused".into());
    folder.interval = "0s".into();
    folder.on_change = false;
    let id = folder.id.clone();
    Config::edit(&path, |c| {
        c.folders.push(folder);
        Ok(())
    })
    .unwrap();
    let (service, events) = Service::start(path.clone()).unwrap();
    wait(&events, |e| matches!(e, ServiceEvent::Config(_)));
    fs::write(&path, "invalid = [").unwrap();
    service.reload();
    wait(&events, |e| matches!(e, ServiceEvent::Warning(_)));
    service.run(&id, 41, Task::Changes);
    wait(&events, |e| {
        matches!(
            e,
            ServiceEvent::Completed {
                request: 41,
                payload: Payload::Changes(_),
                ..
            }
        )
    });
    service.run("missing", 42, Task::History { skip: 0 });
    wait(&events, |e| {
        matches!(e, ServiceEvent::Failed { request: 42, .. })
    });
    stop(service, events);
}
