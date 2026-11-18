use super::*;

fn cat() -> std::process::Command {
    if cfg!(windows) {
        let python = crate::local::acp::fake_agent_interpreter("python3");
        let mut command = crate::process::command(python);
        let copy = "import sys\nwhile chunk := sys.stdin.buffer.raw.read(65536):\n    sys.stdout.buffer.raw.write(chunk)";
        command.args(["-c", copy]);
        command
    } else {
        crate::process::command("cat")
    }
}

async fn kept_cat(owner: Value) -> Kept {
    let command = cat();
    spawn(Keep {
        command: &command,
        io: Io::Pipes { stderr: None },
        retain: LOSSLESS,
        owner,
    })
    .await
    .unwrap()
}

async fn next(lines: &mut KeptLines) -> (String, u64) {
    tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("a line within 10 s")
        .unwrap()
        .expect("output has not ended")
}

#[tokio::test]
async fn a_successor_attaches_where_its_predecessor_acknowledged() {
    let owner = Owner::Terminal {
        terminal_id: "t1".into(),
        state: serde_json::json!({ "projectId": "p" }),
    };
    let mut first = kept_cat(owner.to_value()).await;
    let id = first.handle().id().to_string();
    let mut stdin = first.take_stdin().unwrap();
    let mut lines = first.take_stdout().unwrap().lines();
    stdin.write_all(b"one\ntwo\n").await.unwrap();
    assert_eq!(next(&mut lines).await, ("one".into(), 4));
    assert_eq!(next(&mut lines).await, ("two".into(), 8));
    // Only "one" became durable before this server went away.
    first.handle().ack(4);
    first.handle().detach();
    drop((first, stdin, lines));

    let entry = entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap();
    assert_eq!(entry.liveness, Liveness::Alive);
    assert_eq!(entry.owner, Record::Known(owner));

    let mut second = attach(&id).await.unwrap();
    assert_eq!(second.handle().start(), 4);
    let mut stdin = second.take_stdin().unwrap();
    let mut lines = second.take_stdout().unwrap().lines();
    assert_eq!(next(&mut lines).await, ("two".into(), 8));
    stdin.write_all(b"three\n").await.unwrap();
    assert_eq!(next(&mut lines).await, ("three".into(), 14));

    let exit = second.handle().stop(Duration::from_secs(10)).await.unwrap();
    assert_eq!(exit.reason.as_deref(), Some("stopped"));
    drop(second);
    let gone = async {
        while root().join(&id).exists() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), gone)
        .await
        .expect("a stopped keeper's directory is removed");
}

#[tokio::test]
async fn a_successor_skips_lines_its_checkpoint_already_applied() {
    let mut first = kept_cat(Value::Null).await;
    let id = first.handle().id().to_string();
    let mut stdin = first.take_stdin().unwrap();
    stdin.write_all(b"one\ntwo\n").await.unwrap();
    let mut lines = first.take_stdout().unwrap().lines();
    next(&mut lines).await;
    next(&mut lines).await;
    first.handle().detach();
    drop((first, stdin, lines));

    let mut second = attach(&id).await.unwrap();
    assert_eq!(second.handle().start(), 0);
    let mut stdin = second.take_stdin().unwrap();
    let mut lines = second.take_stdout().unwrap().lines();
    assert!(lines.skip_to(2).is_ok());
    let mid_line = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .unwrap();
    assert!(mid_line.is_err(), "a checkpoint inside a line is refused");

    let mut third = attach(&id).await.unwrap();
    let mut lines = third.take_stdout().unwrap().lines();
    lines.skip_to(8).unwrap();
    stdin = third.take_stdin().unwrap_or(stdin);
    stdin.write_all(b"three\n").await.unwrap();
    assert_eq!(next(&mut lines).await, ("three".into(), 14));
    drop(second);
    third.handle().stop(Duration::from_secs(10)).await.unwrap();
}

#[cfg(unix)]
#[tokio::test]
async fn dropping_a_kept_child_ends_it() {
    let mut kept = kept_cat(Value::Null).await;
    let handle = kept.handle();
    let pid = handle.pid();
    let stdout = kept.take_stdout();
    drop((kept, stdout));
    let alive = |pid: u32| unsafe { libc::kill(pid as i32, 0) } == 0;
    assert!(alive(pid));
    drop(handle);
    let gone = async {
        while alive(pid) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), gone)
        .await
        .expect("the child ends with its last handle");
}

#[tokio::test]
async fn output_ends_with_the_exit_and_flush_waits_for_the_keeper() {
    let mut kept = kept_cat(Value::Null).await;
    let mut stdin = kept.take_stdin().unwrap();
    let mut lines = kept.take_stdout().unwrap().lines();
    stdin.write_all(b"last\n").await.unwrap();
    stdin.flush().await.unwrap();
    kept.handle().close_stdin();
    assert_eq!(next(&mut lines).await, ("last".into(), 5));
    let end = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("output ends once the child exits");
    assert!(end.unwrap().is_none());
    assert_eq!(kept.handle().wait().await.code, Some(0));
}

#[tokio::test]
async fn a_child_held_here_answers_like_a_kept_one_without_a_keeper() {
    let mut held = spawn_here(&mut cat().into(), None).unwrap();
    let mut stdin = held.take_stdin().unwrap();
    let mut lines = held.take_stdout().unwrap().lines();
    stdin.write_all(b"one\n").await.unwrap();
    stdin.flush().await.unwrap();
    assert_eq!(next(&mut lines).await, ("one".into(), 4));
    held.handle().set_owner(&Value::Null).unwrap();
    assert!(entries()
        .unwrap()
        .iter()
        .all(|entry| entry.id != held.handle().id()));
    held.handle().close_stdin();
    let end = tokio::time::timeout(Duration::from_secs(10), lines.next_line())
        .await
        .expect("output ends once the child exits");
    assert!(end.unwrap().is_none());
    assert_eq!(held.handle().wait().await.code, Some(0));
}

#[tokio::test]
async fn a_child_held_here_ends_when_stopped_or_let_go() {
    let held = spawn_here(&mut cat().into(), None).unwrap();
    let exit = held.handle().stop(Duration::from_secs(10)).await.unwrap();
    assert_eq!(exit.reason.as_deref(), Some("stopped"));

    let held = spawn_here(&mut cat().into(), None).unwrap();
    let pid = held.handle().pid();
    held.handle().detach();
    drop(held);
    let gone = async {
        while crate::engine::process::identity(pid).is_some() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), gone)
        .await
        .expect("a child held here ends with its last handle, detached or not");
}

#[cfg(unix)]
fn deny(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000)).unwrap();
    // A privileged user reads past the mode, so nothing can be induced.
    let readable = std::fs::File::open(path).is_ok() || std::fs::read_dir(path).is_ok();
    if readable {
        allow(path);
    }
    !readable
}

#[cfg(unix)]
fn allow(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    let mode = if path.is_dir() { 0o700 } else { 0o600 };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode)).unwrap();
}

fn entry(id: &str) -> Entry {
    entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.id == id)
        .unwrap()
}

#[test]
fn only_a_missing_directory_or_lock_is_a_keeper_that_is_gone() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    assert!(entries().unwrap().is_empty());

    let owner = Owner::Terminal {
        terminal_id: "t".into(),
        state: Value::Null,
    };
    let bare = root().join("bare");
    create_private_dir(&bare).unwrap();
    publish(
        &bare.join(wire::OWNER_FILE),
        &serde_json::to_vec(&owner).unwrap(),
    )
    .unwrap();
    let released = root().join("released");
    create_private_dir(&released).unwrap();
    std::fs::write(released.join(wire::LOCK_FILE), b"").unwrap();

    let bare = entry("bare");
    assert_eq!(bare.liveness, Liveness::Dead);
    assert_eq!(bare.owner, Record::Known(owner));
    assert_eq!(entry("released").liveness, Liveness::Dead);
    assert_eq!(entry("released").owner, Record::Missing);
}

#[cfg(unix)]
#[test]
fn an_unreadable_keeper_directory_is_an_error_not_an_empty_list() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    create_private_dir(&root().join("kept")).unwrap();
    if !deny(&root()) {
        return;
    }
    let error = entries().err().expect("an unlistable root is an error");
    allow(&root());
    assert!(
        error.to_string().contains(&root().display().to_string()),
        "{error}"
    );
    assert_eq!(entries().unwrap().len(), 1);
}

#[cfg(unix)]
#[test]
fn a_keeper_whose_files_cannot_be_read_is_unknown_and_left_in_place() {
    if !crate::config::in_test_child(&[]) {
        return;
    }
    tokio::runtime::Runtime::new().unwrap().block_on(async {
        let owner = Owner::Terminal {
            terminal_id: "t".into(),
            state: Value::Null,
        };
        let kept = kept_cat(owner.to_value()).await;
        let handle = kept.handle();
        let id = handle.id().to_string();
        let dir = root().join(&id);
        let owner_file = dir.join(wire::OWNER_FILE);
        let lock_file = dir.join(wire::LOCK_FILE);

        if !deny(&owner_file) {
            handle.stop(Duration::from_secs(10)).await;
            return;
        }
        let unreadable = entry(&id);
        allow(&owner_file);
        assert_eq!(unreadable.owner, Record::Unreadable);
        assert_eq!(unreadable.liveness, Liveness::Alive);

        std::fs::write(&owner_file, b"{ not json").unwrap();
        assert_eq!(entry(&id).owner, Record::Unreadable);
        std::fs::remove_file(&owner_file).unwrap();
        assert_eq!(entry(&id).owner, Record::Missing);
        handle.set_owner(&owner.to_value()).unwrap();
        assert_eq!(entry(&id).owner, Record::Known(owner));

        assert!(deny(&lock_file));
        let unknown = entry(&id);
        let removal = remove(&id);
        let attachment = attach(&id).await.err();
        allow(&lock_file);
        assert_eq!(unknown.liveness, Liveness::Unknown);
        assert!(removal.is_err() && dir.exists());
        assert!(attachment.is_some());

        assert_eq!(entry(&id).liveness, Liveness::Alive);
        handle.stop(Duration::from_secs(10)).await.unwrap();
    });
}
