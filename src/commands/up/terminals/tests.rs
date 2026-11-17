use std::net::SocketAddr;

use axum::routing::{delete, get};
use axum::Router;
use futures::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::{client::IntoClientRequest as _, Message as Frame};

use super::*;
use crate::commands::up::{close_project_terminal, project_terminal, AppState};

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A command the test shell prints `needle` for, without the command itself containing it.
fn print(word: &str) -> (String, String) {
    if cfg!(windows) {
        (format!("echo {word}-%OS%\r"), format!("{word}-Windows_NT"))
    } else {
        (format!("echo {word}-$((1+1))\r"), format!("{word}-2"))
    }
}

fn add_project(id: &str) -> String {
    let repo = crate::config::test_home().join(id);
    std::fs::create_dir_all(&repo).unwrap();
    Store::open()
        .unwrap()
        .create_local_project(&crate::local::model::LocalProject {
            id: id.into(),
            name: id.into(),
            slug: id.into(),
            github_owner: String::new(),
            github_repo: String::new(),
            github_sync_enabled: false,
            baseline_branch: "main".into(),
            repo_path: repo.to_string_lossy().into_owned(),
            run_command: None,
            paper_id: None,
            created_at: 1,
            updated_at: 1,
        })
        .unwrap();
    id.into()
}

async fn serve(state: AppState) -> SocketAddr {
    let app = Router::new()
        .route("/api/projects/{id}/terminal", get(project_terminal))
        .route(
            "/api/projects/{id}/terminals/{terminal_id}",
            delete(close_project_terminal),
        )
        .with_state(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    address
}

async fn connect(address: SocketAddr, query: &str) -> Socket {
    let mut request = format!("ws://{address}/api/projects/{query}")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    let (mut socket, _) = tokio_tungstenite::connect_async(request).await.unwrap();
    let resize = json!({ "type": "resize", "cols": 100, "rows": 30 }).to_string();
    socket.send(Frame::Text(resize.into())).await.unwrap();
    socket
}

async fn open(address: SocketAddr, project: &str, terminal: &str) -> Socket {
    connect(
        address,
        &format!("{project}/terminal?terminalId={terminal}"),
    )
    .await
}

async fn type_in(socket: &mut Socket, text: &str) {
    let bytes = text.as_bytes().to_vec();
    socket.send(Frame::Binary(bytes.into())).await.unwrap();
}

/// Output up to and including `needle`, or the first text frame when `needle` is `None`.
async fn read_until(socket: &mut Socket, needle: Option<&str>) -> (String, Option<Value>) {
    let mut output = String::new();
    // The first keeper waits on the test build of orx-host.
    tokio::time::timeout(Duration::from_secs(120), async {
        while let Some(frame) = socket.next().await {
            match frame.unwrap() {
                Frame::Binary(bytes) => {
                    output.push_str(&String::from_utf8_lossy(&bytes));
                    if needle.is_some_and(|needle| output.contains(needle)) {
                        return None;
                    }
                }
                Frame::Text(text) => return Some(serde_json::from_str(&text).unwrap()),
                _ => {}
            }
        }
        None
    })
    .await
    .map(|message| (output.clone(), message))
    .unwrap_or_else(|_| panic!("waited for {needle:?}; the terminal showed:\n{output}"))
}

async fn see(socket: &mut Socket, needle: &str) -> String {
    let (output, message) = read_until(socket, Some(needle)).await;
    assert!(
        message.is_none(),
        "{message:?} before {needle:?} in:\n{output}"
    );
    assert!(output.contains(needle), "{needle:?} not in:\n{output}");
    output
}

async fn message(socket: &mut Socket) -> Value {
    read_until(socket, None)
        .await
        .1
        .expect("a message before the socket closed")
}

fn terminal_keepers(id: &str) -> Vec<keep::Entry> {
    keep::entries()
        .into_iter()
        .filter(|entry| {
            matches!(&entry.owner, Some(Owner::Terminal { terminal_id, .. }) if terminal_id == id)
        })
        .collect()
}

async fn delete_terminal(address: SocketAddr, project: &str, id: &str) -> reqwest::StatusCode {
    crate::net::loopback_client()
        .build()
        .unwrap()
        .delete(format!(
            "http://{address}/api/projects/{project}/terminals/{id}"
        ))
        .send()
        .await
        .unwrap()
        .status()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_outlives_its_page_and_ends_with_its_tab() {
    if !crate::config::in_test_child(&["SHELL"]) {
        return;
    }
    let project = add_project("terminal-project");
    let address = serve(crate::commands::up::tests::test_state()).await;

    let mut page = open(address, &project, "tab-1").await;
    let (command, printed) = print("one");
    type_in(&mut page, &command).await;
    see(&mut page, &printed).await;
    if cfg!(unix) {
        type_in(&mut page, "stty size\r").await;
        see(&mut page, "30 100").await;
    }
    drop(page);

    // A reload attaches to the same shell and sees what it printed.
    let mut page = open(address, &project, "tab-1").await;
    see(&mut page, &printed).await;
    let (command, printed) = print("two");
    type_in(&mut page, &command).await;
    see(&mut page, &printed).await;
    assert_eq!(terminal_keepers("tab-1").len(), 1);

    // A second page takes the shell over; the first is told why it lost it.
    let mut other = open(address, &project, "tab-1").await;
    assert_eq!(message(&mut page).await["type"], "elsewhere");
    see(&mut other, &printed).await;

    type_in(&mut other, "exit 3\r").await;
    assert_eq!(
        message(&mut other).await,
        json!({ "type": "exit", "code": 3 })
    );
    let mut page = open(address, &project, "tab-1").await;
    assert_eq!(
        read_until(&mut page, None).await.1,
        Some(json!({ "type": "exit", "code": 3 }))
    );

    // Closing the tab ends it; the id then starts a fresh shell.
    assert_eq!(delete_terminal(address, &project, "tab-1").await, 200);
    let mut page = open(address, &project, "tab-1").await;
    let (command, printed) = print("three");
    type_in(&mut page, &command).await;
    let output = see(&mut page, &printed).await;
    assert!(!output.contains("two-"), "{output}");
    assert_eq!(delete_terminal(address, &project, "tab-1").await, 200);
    let gone = async {
        while !terminal_keepers("tab-1").is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), gone)
        .await
        .expect("a closed terminal's keeper goes away");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_terminal_is_only_its_projects() {
    if !crate::config::in_test_child(&["SHELL"]) {
        return;
    }
    let mine = add_project("mine");
    let theirs = add_project("theirs");
    let address = serve(crate::commands::up::tests::test_state()).await;
    let mut page = open(address, &mine, "shared-id").await;
    let (command, printed) = print("mine");
    type_in(&mut page, &command).await;
    see(&mut page, &printed).await;

    let mut intruder = open(address, &theirs, "shared-id").await;
    let refused = message(&mut intruder).await;
    assert_eq!(refused["type"], "error", "{refused}");
    assert_eq!(delete_terminal(address, &theirs, "shared-id").await, 404);
    assert_eq!(
        delete_terminal(address, "theirs", "../shared-id").await,
        404
    );

    let mut request = format!("ws://{address}/api/projects/{mine}/terminal?terminalId=a%20b")
        .into_client_request()
        .unwrap();
    request
        .headers_mut()
        .insert("origin", format!("http://{address}").parse().unwrap());
    let refused = tokio_tungstenite::connect_async(request).await.unwrap_err();
    assert!(
        matches!(&refused, tokio_tungstenite::tungstenite::Error::Http(response) if response.status() == 400),
        "{refused}"
    );

    let unknown = open(address, "no-such-project", "new-id").await;
    let mut unknown = unknown;
    assert_eq!(message(&mut unknown).await["type"], "error");
    assert!(terminal_keepers("new-id").is_empty());

    // The shell outlives a page that was refused another project's terminal.
    let (command, printed) = print("still");
    type_in(&mut page, &command).await;
    see(&mut page, &printed).await;
    assert_eq!(delete_terminal(address, &mine, "shared-id").await, 200);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_without_a_terminal_id_gets_a_shell_that_ends_with_its_socket() {
    if !crate::config::in_test_child(&["SHELL"]) {
        return;
    }
    let project = add_project("legacy");
    let address = serve(crate::commands::up::tests::test_state()).await;
    let mut page = connect(address, &format!("{project}/terminal")).await;
    let (command, printed) = print("old");
    type_in(&mut page, &command).await;
    see(&mut page, &printed).await;
    assert!(keep::entries()
        .iter()
        .all(|entry| !matches!(entry.owner, Some(Owner::Terminal { .. }))));
    drop(page);

    let mut page = connect(address, &format!("{project}/terminal")).await;
    let (command, printed) = print("new");
    type_in(&mut page, &command).await;
    let output = see(&mut page, &printed).await;
    assert!(!output.contains("old-"), "{output}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_next_server_takes_back_live_terminals() {
    if !crate::config::in_test_child(&["SHELL"]) {
        return;
    }
    let project = add_project("adopted");
    let doomed = add_project("deleted-meanwhile");
    let first = crate::commands::up::tests::test_state();
    let address = serve(first.clone()).await;
    let mut page = open(address, &project, "survivor").await;
    let (command, printed) = print("before");
    type_in(&mut page, &command).await;
    see(&mut page, &printed).await;
    let mut orphan = open(address, &doomed, "orphan").await;
    let (command, printed_there) = print("doomed");
    type_in(&mut orphan, &command).await;
    see(&mut orphan, &printed_there).await;

    // A restart: this server lets go of its terminals, and its project goes away meanwhile.
    first.terminals.release();
    drop((page, orphan));
    Store::open()
        .unwrap()
        .delete_local_project(&doomed)
        .unwrap();
    let ended = left_behind("ended-unattended", Some("orphaned"));
    let stopped = left_behind("stopped-on-quit", Some("stopped"));

    let second = crate::commands::up::tests::test_state();
    let mut entries = terminal_keepers("survivor");
    entries.extend(terminal_keepers("orphan"));
    entries.extend(terminal_keepers("ended-unattended"));
    entries.extend(terminal_keepers("stopped-on-quit"));
    second.terminals.adopt(entries).await;
    let address = serve(second.clone()).await;

    let mut page = open(address, &project, "survivor").await;
    see(&mut page, &printed).await;
    let (command, printed) = print("after");
    type_in(&mut page, &command).await;
    see(&mut page, &printed).await;

    let mut page = open(address, &project, "ended-unattended").await;
    assert_eq!(message(&mut page).await["type"], "gone");
    assert!(!keep::root().join(ended).exists());
    assert!(!keep::root().join(stopped).exists());
    let mut page = open(address, &project, "stopped-on-quit").await;
    let (command, printed) = print("fresh");
    type_in(&mut page, &command).await;
    see(&mut page, &printed).await;

    let stopped = async {
        while terminal_keepers("orphan").iter().any(|entry| entry.alive) {
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(10), stopped)
        .await
        .expect("a terminal whose project is gone is stopped");
    second.terminals.shutdown().await;
    assert!(terminal_keepers("survivor")
        .iter()
        .all(|entry| !entry.alive));
}

/// A keeper directory whose keeper ended its terminal and exited, as `reason` says.
fn left_behind(terminal_id: &str, reason: Option<&str>) -> String {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let dir = keep::root().join(&id);
    std::fs::create_dir_all(&dir).unwrap();
    let owner = Owner::Terminal {
        terminal_id: terminal_id.into(),
        state: json!({ "projectId": "adopted", "sessionId": null, "cols": 80, "rows": 24, "cwd": "/" }),
    };
    std::fs::write(
        dir.join(orx_keep::wire::OWNER_FILE),
        serde_json::to_vec(&owner.to_value()).unwrap(),
    )
    .unwrap();
    let exit = json!({ "code": null, "signal": 9, "finishedAt": 1, "reason": reason });
    std::fs::write(dir.join(orx_keep::wire::EXIT_FILE), exit.to_string()).unwrap();
    id
}
