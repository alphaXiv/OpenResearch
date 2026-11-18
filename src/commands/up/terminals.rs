//! Project terminals: each a shell in a keeper, attached to by this server for as long as it
//! runs, so a page that reloads or comes back after a restart returns to the same shell.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use axum::extract::ws::{Message, WebSocket};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt as _;
use tokio::sync::mpsc;

use super::{apply_resize, close_socket, send_terminal_error, AppState, DEFAULT_PTY_SIZE};
use crate::error::{anyhow, Result};
use crate::keep::{
    self, Exit, Io, Keep, Kept, KeptHandle, KeptStdin, KeptStdout, Liveness, Owner, Record, Retain,
};
use crate::store::Store;

/// Output a terminal replays to a page that attaches, held by its keeper and by this server.
const SCROLLBACK: usize = 2 << 20;
const CURSOR_QUERY: &[u8] = b"\x1b[6n";
const STOP_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Default)]
pub(super) struct Terminals {
    live: Mutex<HashMap<String, Arc<Terminal>>>,
    /// What to tell a page about a terminal that ended while no server was attached.
    ended: Mutex<HashMap<String, Value>>,
    /// Held while finding or starting a terminal, so one id never gets two shells.
    opening: tokio::sync::Mutex<()>,
}

/// `Owner::Terminal`'s state.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TerminalState {
    project_id: String,
    session_id: Option<String>,
    cols: u16,
    rows: u16,
    cwd: String,
}

struct Terminal {
    project_id: String,
    session_id: Option<String>,
    handle: KeptHandle,
    input: tokio::sync::Mutex<KeptStdin>,
    view: Mutex<View>,
    /// Cursor queries before this offset were answered before this server attached: by an
    /// earlier server, or on Windows by the keeper.
    answered_until: u64,
}

#[derive(Default)]
struct View {
    scrollback: VecDeque<u8>,
    viewer: Option<Viewer>,
    viewers: u64,
    /// A cursor-position query nobody has answered; ConPTY holds the shell's output until then.
    cursor_query: bool,
    exit: Option<Exit>,
}

struct Viewer {
    generation: u64,
    frames: mpsc::Sender<Frame>,
}

enum Frame {
    Output(Vec<u8>),
    Exit(Exit),
}

enum Opened {
    Live(Arc<Terminal>, Subscription),
    Ended(Value),
}

/// A page's place as a terminal's viewer: what to replay, then live frames.
struct Subscription {
    generation: u64,
    replay: Vec<u8>,
    exit: Option<Exit>,
    frames: mpsc::Receiver<Frame>,
}

impl Terminals {
    /// Serves one page's socket for terminal `id`, starting the shell when there is none.
    pub(super) async fn serve(
        &self,
        state: &AppState,
        mut socket: WebSocket,
        project_id: String,
        session_id: Option<String>,
        id: String,
        shell: (String, Vec<String>),
    ) {
        match self
            .open_unless_moving(state, &project_id, session_id, &id, shell)
            .await
        {
            Ok(Opened::Live(terminal, subscription)) => terminal.view(socket, subscription).await,
            Ok(Opened::Ended(message)) => {
                let _ = socket.send(Message::Text(message.to_string().into())).await;
                close_socket(&mut socket).await;
            }
            Err(error) => send_terminal_error(&mut socket, error).await,
        }
    }

    /// A data-directory move refuses while a terminal's keeper lives there, so a shell may only
    /// start while none is under way, and the move's check must then find it.
    async fn open_unless_moving(
        &self,
        state: &AppState,
        project_id: &str,
        session_id: Option<String>,
        id: &str,
        shell: (String, Vec<String>),
    ) -> Result<Opened> {
        let moving = |state| super::reject_if_moving(state).map_err(|error| anyhow!(error.1));
        moving(state)?;
        let _data = state.data_dir_gate.lock().await;
        moving(state)?;
        self.open(project_id, session_id, id, shell).await
    }

    async fn open(
        &self,
        project_id: &str,
        session_id: Option<String>,
        id: &str,
        shell: (String, Vec<String>),
    ) -> Result<Opened> {
        let _opening = self.opening.lock().await;
        if let Some(message) = self.ended.lock().unwrap().get(id) {
            return Ok(Opened::Ended(message.clone()));
        }
        let existing = self.live.lock().unwrap().get(id).cloned();
        if let Some(terminal) = existing {
            if terminal.project_id != project_id {
                return Err(anyhow!("This terminal belongs to another project."));
            }
            if terminal.session_id == session_id {
                let subscription = terminal.subscribe();
                return Ok(Opened::Live(terminal, subscription));
            }
            // The tab's task became a chat: its shell moves to the chat's checkout.
            self.live.lock().unwrap().remove(id);
            terminal.handle.stop(STOP_TIMEOUT).await;
        }

        let root = {
            let project_id = project_id.to_string();
            let session_id = session_id.clone();
            tokio::task::spawn_blocking(move || {
                super::project_terminal_root(&project_id, session_id.as_deref())
            })
            .await
            .map_err(|error| anyhow!("Could not find the terminal's folder: {error}"))??
        };
        let (terminal, output) = spawn(id, project_id, session_id, root, shell).await?;
        // Subscribed before any output, so the page that started the shell answers its first
        // cursor query.
        let subscription = terminal.subscribe();
        tokio::spawn(pump(Arc::downgrade(&terminal), output));
        self.live
            .lock()
            .unwrap()
            .insert(id.to_string(), terminal.clone());
        Ok(Opened::Live(terminal, subscription))
    }

    /// Ends terminal `id` for good, as closing its tab does. False when it is another project's.
    pub(super) async fn close(&self, project_id: &str, id: &str) -> bool {
        // Waits out an open in flight, which would otherwise start its shell after the tab is gone.
        let opening = self.opening.lock().await;
        let terminal = {
            let mut live = self.live.lock().unwrap();
            match live.get(id) {
                Some(terminal) if terminal.project_id != project_id => return false,
                Some(_) => live.remove(id),
                None => None,
            }
        };
        self.ended.lock().unwrap().remove(id);
        drop(opening);
        if let Some(terminal) = terminal {
            terminal.handle.stop(STOP_TIMEOUT).await;
        }
        true
    }

    /// Ends the terminals of a project or chat that is going away, before its folders do.
    pub(super) async fn close_where(&self, matches: impl Fn(&str, Option<&str>) -> bool) {
        let opening = self.opening.lock().await;
        let closing: Vec<_> = {
            let mut live = self.live.lock().unwrap();
            let ids: Vec<_> = live
                .iter()
                .filter(|(_, terminal)| {
                    matches(&terminal.project_id, terminal.session_id.as_deref())
                })
                .map(|(id, _)| id.clone())
                .collect();
            ids.iter().filter_map(|id| live.remove(id)).collect()
        };
        drop(opening);
        futures::future::join_all(
            closing
                .iter()
                .map(|terminal| terminal.handle.stop(STOP_TIMEOUT)),
        )
        .await;
    }

    /// Takes back the terminals a predecessor left: live ones are attached to again, ones whose
    /// project or chat is gone are stopped, and ones that ended unattended are remembered.
    pub(super) async fn adopt(&self, entries: Vec<keep::Entry>) {
        for entry in entries {
            let Record::Known(Owner::Terminal { terminal_id, state }) = entry.owner else {
                continue;
            };
            // Logged when listed; its shell may be running, so it stays as it is.
            if entry.liveness == Liveness::Unknown {
                continue;
            }
            if entry.liveness == Liveness::Dead {
                // A stop was this server's own doing: the page gets a fresh shell.
                if entry.exit.as_ref().and_then(|exit| exit.reason.as_deref()) != Some("stopped") {
                    let message = entry
                        .exit
                        .as_ref()
                        .map_or_else(|| json!({ "type": "gone" }), exit_message);
                    self.ended.lock().unwrap().insert(terminal_id, message);
                }
                if let Err(error) = keep::remove(&entry.id) {
                    eprintln!("orx up: {error:#}");
                }
                continue;
            }
            let kept = match keep::attach(&entry.id).await {
                Ok(kept) => kept,
                Err(error) => {
                    eprintln!("orx up: could not reattach terminal {terminal_id}: {error:#}");
                    let gone = json!({ "type": "gone" });
                    self.ended.lock().unwrap().insert(terminal_id, gone);
                    continue;
                }
            };
            let state = match serde_json::from_value::<TerminalState>(state) {
                Ok(state) => state,
                Err(error) => {
                    eprintln!("orx up: stopping terminal {terminal_id} with an unreadable record: {error}");
                    kept.handle().stop(STOP_TIMEOUT).await;
                    continue;
                }
            };
            let (project_id, session_id) = (state.project_id.clone(), state.session_id.clone());
            let owned = tokio::task::spawn_blocking(move || owner_exists(&project_id, session_id))
                .await
                .map_err(|error| anyhow!("{error}"))
                .and_then(|result| result);
            match owned {
                Ok(false) => {
                    kept.handle().stop(STOP_TIMEOUT).await;
                    continue;
                }
                // Unknown is not absent: an unreadable store keeps the shell.
                Err(error) => {
                    eprintln!("orx up: keeping terminal {terminal_id}; could not check its project: {error:#}")
                }
                Ok(true) => {}
            }
            let answered_until = kept.handle().next();
            let (terminal, output) =
                Terminal::new(kept, state.project_id, state.session_id, answered_until);
            tokio::spawn(pump(Arc::downgrade(&terminal), output));
            self.live.lock().unwrap().insert(terminal_id, terminal);
        }
    }

    /// Stops every terminal on quit; a hand-off leaves them to the next server.
    pub(super) async fn shutdown(&self) {
        if keep::handing_off() {
            self.release();
            return;
        }
        self.close_where(|_, _| true).await;
    }

    /// Lets go of every terminal without ending it.
    fn release(&self) {
        for (_, terminal) in self.live.lock().unwrap().drain() {
            terminal.handle.detach();
        }
    }
}

fn owner_exists(project_id: &str, session_id: Option<String>) -> Result<bool> {
    let store = Store::open()?;
    if store.get_local_project(project_id)?.is_none() {
        return Ok(false);
    }
    Ok(match session_id {
        Some(id) => store
            .get_chat_session(&id)?
            .is_some_and(|session| session.project_id == project_id),
        None => true,
    })
}

async fn spawn(
    id: &str,
    project_id: &str,
    session_id: Option<String>,
    root: PathBuf,
    (shell, args): (String, Vec<String>),
) -> Result<(Arc<Terminal>, KeptStdout)> {
    let mut command = crate::process::command(&shell);
    command.args(args).current_dir(&root);
    for (key, value) in super::terminal_env() {
        match value {
            Some(value) => command.env(key, value),
            None => command.env_remove(key),
        };
    }
    let (cols, rows) = (DEFAULT_PTY_SIZE.cols, DEFAULT_PTY_SIZE.rows);
    let state = TerminalState {
        project_id: project_id.to_string(),
        session_id: session_id.clone(),
        cols,
        rows,
        cwd: root.to_string_lossy().into_owned(),
    };
    let owner = Owner::Terminal {
        terminal_id: id.to_string(),
        state: serde_json::to_value(state)?,
    };
    let kept = keep::spawn(Keep {
        command: &command,
        io: Io::Pty { cols, rows },
        retain: Retain::Tail {
            bytes: SCROLLBACK as u64,
        },
        owner: owner.to_value(),
    })
    .await
    .map_err(|error| anyhow!("Could not start {shell} in {}: {error}", root.display()))?;
    let answered_until = if cfg!(windows) {
        kept.handle().next()
    } else {
        0
    };
    Ok(Terminal::new(
        kept,
        project_id.to_string(),
        session_id,
        answered_until,
    ))
}

impl Terminal {
    fn new(
        mut kept: Kept,
        project_id: String,
        session_id: Option<String>,
        answered_until: u64,
    ) -> (Arc<Self>, KeptStdout) {
        let input = kept.take_stdin().expect("a new attachment has its stdin");
        let output = kept.take_stdout().expect("a new attachment has its stdout");
        let terminal = Arc::new(Self {
            project_id,
            session_id,
            answered_until,
            handle: kept.handle(),
            input: tokio::sync::Mutex::new(input),
            view: Mutex::default(),
        });
        (terminal, output)
    }

    async fn write(&self, bytes: &[u8]) {
        let mut input = self.input.lock().await;
        self.view.lock().unwrap().cursor_query = false;
        // A keeper that is gone reports the shell's exit through the output.
        let _ = input.write_all(bytes).await;
    }

    async fn output(&self, offset: u64, bytes: Vec<u8>) {
        let (frames, live, answer) = {
            let mut view = self.view.lock().unwrap();
            // A query answered twice reaches the shell as input, which ConPTY reads as F3.
            let mut live = Vec::with_capacity(bytes.len());
            let mut rest = &bytes[..];
            let mut at_offset = offset;
            while let Some(at) = cursor_query(rest) {
                view.scrollback.extend(&rest[..at]);
                live.extend_from_slice(&rest[..at]);
                if at_offset + at as u64 >= self.answered_until {
                    view.cursor_query = true;
                    live.extend_from_slice(CURSOR_QUERY);
                }
                at_offset += (at + CURSOR_QUERY.len()) as u64;
                rest = &rest[at + CURSOR_QUERY.len()..];
            }
            view.scrollback.extend(rest);
            live.extend_from_slice(rest);
            let excess = view.scrollback.len().saturating_sub(SCROLLBACK);
            view.scrollback.drain(..excess);
            match &view.viewer {
                Some(viewer) => (Some(viewer.frames.clone()), live, false),
                None => (None, live, std::mem::take(&mut view.cursor_query)),
            }
        };
        if let Some(frames) = frames {
            // A viewer that left answers the query as it unsubscribes.
            let _ = frames.send(Frame::Output(live)).await;
        }
        if answer {
            self.write(b"\x1b[1;1R").await;
        }
    }

    async fn exited(&self, exit: Exit) {
        let frames = {
            let mut view = self.view.lock().unwrap();
            view.exit = Some(exit.clone());
            view.viewer.as_ref().map(|viewer| viewer.frames.clone())
        };
        if let Some(frames) = frames {
            let _ = frames.send(Frame::Exit(exit)).await;
        }
        // A keeper waits on its attachment after the shell exits; let it go.
        self.handle.stop(STOP_TIMEOUT).await;
    }

    /// Makes the caller this terminal's only viewer.
    fn subscribe(&self) -> Subscription {
        let (sender, frames) = mpsc::channel(64);
        let mut view = self.view.lock().unwrap();
        view.viewers += 1;
        let generation = view.viewers;
        view.viewer = Some(Viewer {
            generation,
            frames: sender,
        });
        let mut replay: Vec<u8> = view.scrollback.iter().copied().collect();
        // The page this one replaces may have left without answering; this page answers it.
        if view.cursor_query {
            replay.extend_from_slice(CURSOR_QUERY);
        }
        Subscription {
            generation,
            replay,
            exit: view.exit.clone(),
            frames,
        }
    }

    /// Shows the terminal on `socket`: the scrollback, then live output until either side ends.
    async fn view(&self, mut socket: WebSocket, subscription: Subscription) {
        let Subscription {
            generation,
            replay,
            exit,
            mut frames,
        } = subscription;
        if replay.is_empty() || socket.send(Message::Binary(replay.into())).await.is_ok() {
            match exit {
                Some(exit) => send_exit(&mut socket, &exit).await,
                None => self.relay(&mut socket, &mut frames).await,
            }
        }
        self.unsubscribe(generation).await;
    }

    async fn relay(&self, socket: &mut WebSocket, frames: &mut mpsc::Receiver<Frame>) {
        loop {
            tokio::select! {
                frame = frames.recv() => match frame {
                    Some(Frame::Output(bytes)) => {
                        if socket.send(Message::Binary(bytes.into())).await.is_err() {
                            return;
                        }
                    }
                    Some(Frame::Exit(exit)) => return send_exit(socket, &exit).await,
                    // Only another viewer takes the channel away.
                    None => {
                        let elsewhere = json!({ "type": "elsewhere" }).to_string();
                        let _ = socket.send(Message::Text(elsewhere.into())).await;
                        return close_socket(socket).await;
                    }
                },
                message = socket.recv() => match message {
                    Some(Ok(Message::Binary(bytes))) => self.write(&bytes).await,
                    Some(Ok(Message::Text(text))) => {
                        let mut size = DEFAULT_PTY_SIZE;
                        if apply_resize(&mut size, &text) {
                            self.handle.resize(size.cols, size.rows);
                        }
                    }
                    Some(Ok(Message::Close(_))) | None | Some(Err(_)) => return,
                    Some(Ok(_)) => {}
                },
            }
        }
    }

    async fn unsubscribe(&self, generation: u64) {
        let answer = {
            let mut view = self.view.lock().unwrap();
            if view
                .viewer
                .as_ref()
                .is_some_and(|viewer| viewer.generation == generation)
            {
                view.viewer = None;
                std::mem::take(&mut view.cursor_query)
            } else {
                false
            }
        };
        if answer {
            self.write(b"\x1b[1;1R").await;
        }
    }
}

fn cursor_query(bytes: &[u8]) -> Option<usize> {
    bytes
        .windows(CURSOR_QUERY.len())
        .position(|window| window == CURSOR_QUERY)
}

/// Reads the shell's output for as long as the terminal is registered, viewed or not.
async fn pump(terminal: Weak<Terminal>, mut output: KeptStdout) {
    let exit = loop {
        let Some(handle) = terminal.upgrade().map(|terminal| terminal.handle.clone()) else {
            return;
        };
        tokio::select! {
            biased;
            chunk = output.chunk() => match chunk {
                Some((offset, bytes)) => match terminal.upgrade() {
                    Some(terminal) => terminal.output(offset, bytes).await,
                    None => return,
                },
                None => break handle.wait().await,
            },
            // A keeper holds its attachment open after the exit, so the output does not end.
            exit = handle.wait() => break exit,
        }
    };
    if let Some(terminal) = terminal.upgrade() {
        terminal.exited(exit).await;
    }
}

/// An exit of the shell's own carries its code; one orx caused or lost does not.
fn exit_message(exit: &Exit) -> Value {
    let code = exit
        .code
        .or(exit.signal.map(|signal| 128 + signal))
        .filter(|_| exit.reason.is_none());
    match code {
        Some(code) => json!({ "type": "exit", "code": code }),
        None => json!({ "type": "gone" }),
    }
}

async fn send_exit(socket: &mut WebSocket, exit: &Exit) {
    let message = exit_message(exit).to_string();
    let _ = socket.send(Message::Text(message.into())).await;
    close_socket(socket).await;
}

#[cfg(test)]
mod tests;
