use super::*;

fn cat() -> std::process::Command {
    crate::process::command("cat")
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

#[cfg(unix)]
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

    let entry = entries().into_iter().find(|entry| entry.id == id).unwrap();
    assert!(entry.alive);
    assert_eq!(entry.owner, Some(owner));

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

#[cfg(unix)]
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
