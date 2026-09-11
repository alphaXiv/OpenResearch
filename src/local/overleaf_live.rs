//! Keep a paper and an Overleaf project in step *as people type*.
//!
//! `crate::local::overleaf` works through the git bridge, which snapshots a
//! project only when asked and rate-limits anyone who asks often. The channel
//! Overleaf's own editor uses has neither limit: a socket.io (0.9) connection
//! that streams each document's operational-transform ops as they happen. This
//! module speaks that channel in both directions — an edit on Overleaf lands in
//! the checkout within a fraction of a second, and a save here is diffed into
//! an op and sent back the same way.
//!
//! The channel authenticates with the user's browser session cookie, not the
//! git token, and it is not a documented API: the protocol here follows the
//! open-source Overleaf sources (`services/real-time`, the `sharejs` text
//! type) and is expected to need attention when they change.
//!
//! Only text documents travel this way (`.tex`, `.bib`, and the class/style
//! files); figures still arrive through the git sync. The handshake relies on
//! the project being joined from its id in the query, as overleaf.com does;
//! an older Server Pro that waits for an explicit join stays "connecting".

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock, Mutex, Once, OnceLock};
use std::time::{Duration, Instant, SystemTime};

use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tokio::sync::Notify;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

use crate::error::{anyhow, Result};
use crate::local::overleaf::{
    confined_path, hash, text_doc, to_checkout, write_pulled, Baseline, Project, CLOUD_HOST,
};

/// The cookie www.overleaf.com sets; a Server Pro site may use another name,
/// which is why a pasted `name=value` is kept as written.
const DEFAULT_COOKIE_NAME: &str = "overleaf_session2";

/// How often the checkout is looked at for a local edit, and dirty documents
/// are flushed to disk. Stat-only unless something moved.
const TICK: Duration = Duration::from_millis(250);

/// An op the server has not echoed back in this long is presumed lost; the
/// document is rejoined rather than resent, so nothing lands twice.
const INFLIGHT_TIMEOUT: Duration = Duration::from_secs(10);

/// A dashboard tab keeps its session alive by asking for it again; one that has
/// not asked in this long has closed, and its socket goes with it.
const IDLE_TIMEOUT: Duration = Duration::from_secs(150);

/// Overleaf answers a burst of project joins with a rate-limit rejection and
/// its own client waits this long before trying again.
const RATE_LIMIT_PAUSE: Duration = Duration::from_secs(15);

const MAX_BACKOFF: Duration = Duration::from_secs(60);
const RETRY_AFTER_NETWORK_ERROR: Duration = Duration::from_secs(5);

// --- session cookie ----------------------------------------------------------

/// The stored cookie and the host it is for.
pub fn session() -> Option<(String, String)> {
    crate::config::overleaf_session()
}

/// Accepts what the user is likely to have on the clipboard: the bare value,
/// `name=value`, or a whole `Cookie:` header. Stored as `name=value` pairs,
/// tied to one host the way the git token is tied to one bridge.
pub fn set_session(host: &str, raw: &str) -> Result<()> {
    let normalized = normalize_session(raw)?;
    let host = host.trim().to_ascii_lowercase();
    if host.is_empty() || host.contains(['/', ' ']) {
        return Err(anyhow!("That does not look like an Overleaf host name."));
    }
    let host = if host == "overleaf.com" {
        CLOUD_HOST.to_string()
    } else {
        host
    };
    crate::config::set_overleaf_session(&host, &normalized)?;
    stop_all();
    Ok(())
}

pub fn clear_session() -> Result<()> {
    crate::config::clear_overleaf_session()?;
    stop_all();
    Ok(())
}

fn normalize_session(raw: &str) -> Result<String> {
    let raw = raw.trim();
    let raw = raw
        .strip_prefix("Cookie:")
        .or_else(|| raw.strip_prefix("cookie:"))
        .unwrap_or(raw)
        .trim();
    if raw.is_empty() || raw.lines().count() != 1 {
        return Err(anyhow!("That does not look like a session cookie."));
    }
    let pairs: Vec<String> = raw
        .split(';')
        .map(str::trim)
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) if !name.trim().is_empty() => {
                format!("{}={}", name.trim(), value.trim())
            }
            _ => format!("{DEFAULT_COOKIE_NAME}={pair}"),
        })
        .collect();
    if pairs.iter().any(|pair| pair.contains(char::is_whitespace)) {
        return Err(anyhow!(
            "That does not look like a session cookie — it contains spaces. Copy the value of the {DEFAULT_COOKIE_NAME} cookie from your browser."
        ));
    }
    Ok(pairs.join("; "))
}

// --- socket.io 0.9 framing ----------------------------------------------------

/// `<type>:<id>[+]:<endpoint>[:<data>]`. Only the packets this client acts on
/// are named; everything else is `Other`.
#[derive(Debug, PartialEq)]
enum Packet {
    Disconnect,
    Connect,
    Heartbeat,
    Event { name: String, args: Vec<Value> },
    Ack { id: u64, args: Vec<Value> },
    Error { reason: String },
    Other,
}

fn parse_packet(frame: &str) -> Option<Packet> {
    let mut parts = frame.splitn(4, ':');
    let kind = parts.next()?;
    let _id = parts.next()?;
    let _endpoint = parts.next()?;
    let data = parts.next().unwrap_or("");
    Some(match kind {
        "0" => Packet::Disconnect,
        "1" => Packet::Connect,
        "2" => Packet::Heartbeat,
        "5" => {
            let value: Value = serde_json::from_str(data).ok()?;
            let name = value.get("name")?.as_str()?.to_string();
            let args = value
                .get("args")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            Packet::Event { name, args }
        }
        "6" => {
            // `6:::<id>` or `6:::<id>+[args]`
            let (id, rest) = data.split_once('+').unwrap_or((data, ""));
            let id = id.trim().parse().ok()?;
            let args = if rest.is_empty() {
                Vec::new()
            } else {
                serde_json::from_str::<Value>(rest)
                    .ok()?
                    .as_array()
                    .cloned()
                    .unwrap_or_default()
            };
            Packet::Ack { id, args }
        }
        "7" => Packet::Error {
            reason: data.to_string(),
        },
        _ => Packet::Other,
    })
}

fn event_frame(id: u64, name: &str, args: &[Value]) -> String {
    format!("5:{id}+::{}", json!({ "name": name, "args": args }))
}

const HEARTBEAT_FRAME: &str = "2::";

// --- text OT ------------------------------------------------------------------

/// Overleaf positions index the JavaScript string, so documents are held as
/// UTF-16 code units and only become text at the edge.
type Text = Vec<u16>;

#[derive(Clone, Debug, PartialEq, Eq)]
enum Component {
    Insert { p: usize, text: Text },
    Delete { p: usize, text: Text },
}

fn to_text(s: &str) -> Text {
    s.encode_utf16().collect()
}

fn from_text(t: &[u16]) -> String {
    String::from_utf16_lossy(t)
}

/// Comment components (`c`) are skipped: they carry no text change. Anything
/// else unrecognised is an error, since applying half an op corrupts the doc.
fn parse_op(value: &Value) -> Result<Vec<Component>> {
    let items = value
        .as_array()
        .ok_or_else(|| anyhow!("op is not a list"))?;
    let mut op = Vec::with_capacity(items.len());
    for item in items {
        let p = item
            .get("p")
            .and_then(Value::as_u64)
            .and_then(|p| usize::try_from(p).ok())
            .ok_or_else(|| anyhow!("op component without a position"))?;
        if let Some(text) = item.get("i").and_then(Value::as_str) {
            op.push(Component::Insert {
                p,
                text: to_text(text),
            });
        } else if let Some(text) = item.get("d").and_then(Value::as_str) {
            op.push(Component::Delete {
                p,
                text: to_text(text),
            });
        } else if item.get("c").is_some() {
            continue;
        } else {
            return Err(anyhow!("unknown op component"));
        }
    }
    Ok(op)
}

fn op_json(op: &[Component]) -> Value {
    Value::Array(
        op.iter()
            .map(|c| match c {
                Component::Insert { p, text } => json!({ "p": p, "i": from_text(text) }),
                Component::Delete { p, text } => json!({ "p": p, "d": from_text(text) }),
            })
            .collect(),
    )
}

fn apply(text: &mut Text, op: &[Component]) -> Result<()> {
    for component in op {
        match component {
            Component::Insert { p, text: inserted } => {
                if *p > text.len() {
                    return Err(anyhow!("insert past the end of the document"));
                }
                text.splice(p..p, inserted.iter().copied());
            }
            Component::Delete { p, text: deleted } => {
                let end = p + deleted.len();
                if end > text.len() || text[*p..end] != deleted[..] {
                    return Err(anyhow!("delete does not match the document"));
                }
                text.drain(*p..end);
            }
        }
    }
    Ok(())
}

fn transform_position(pos: usize, other: &Component, insert_after: bool) -> usize {
    match other {
        Component::Insert { p, text } => {
            if *p < pos || (*p == pos && insert_after) {
                pos + text.len()
            } else {
                pos
            }
        }
        Component::Delete { p, text } => {
            if pos <= *p {
                pos
            } else if pos <= p + text.len() {
                *p
            } else {
                pos - text.len()
            }
        }
    }
}

/// sharejs's `text.transformComponent`: `c` rewritten to apply after `other`.
/// `right_side` breaks the tie between two inserts at one position.
fn transform_component(
    dest: &mut Vec<Component>,
    c: &Component,
    other: &Component,
    right_side: bool,
) {
    match c {
        Component::Insert { p, text } => dest.push(Component::Insert {
            p: transform_position(*p, other, right_side),
            text: text.clone(),
        }),
        Component::Delete { p, text } => match other {
            Component::Insert {
                p: other_p,
                text: inserted,
            } => {
                let mut rest = text.as_slice();
                if *p < *other_p {
                    let head = (other_p - p).min(rest.len());
                    dest.push(Component::Delete {
                        p: *p,
                        text: rest[..head].to_vec(),
                    });
                    rest = &rest[head..];
                }
                if !rest.is_empty() {
                    dest.push(Component::Delete {
                        p: p + inserted.len(),
                        text: rest.to_vec(),
                    });
                }
            }
            Component::Delete {
                p: other_p,
                text: other_text,
            } => {
                let other_end = other_p + other_text.len();
                let end = p + text.len();
                if *p >= other_end {
                    dest.push(Component::Delete {
                        p: p - other_text.len(),
                        text: text.clone(),
                    });
                } else if end <= *other_p {
                    dest.push(c.clone());
                } else {
                    let mut kept = Text::new();
                    if *p < *other_p {
                        kept.extend_from_slice(&text[..other_p - p]);
                    }
                    if end > other_end {
                        kept.extend_from_slice(&text[other_end - p..]);
                    }
                    if !kept.is_empty() {
                        dest.push(Component::Delete {
                            p: transform_position(*p, other, false),
                            text: kept,
                        });
                    }
                }
            }
        },
    }
}

/// sharejs's `transformX`: both ops rewritten so each applies after the other.
/// `left` is ours and wins position ties, as the reference client's does.
fn transform_x(left: &[Component], right: &[Component]) -> (Vec<Component>, Vec<Component>) {
    let mut left: Vec<Component> = left.to_vec();
    let mut new_right = Vec::new();
    for rc in right {
        let mut right_c = Some(rc.clone());
        let mut new_left = Vec::new();
        let mut k = 0;
        while k < left.len() {
            let Some(current) = right_c.as_ref() else {
                break;
            };
            let mut next = Vec::new();
            transform_component(&mut new_left, &left[k], current, false);
            transform_component(&mut next, current, &left[k], true);
            k += 1;
            match next.len() {
                1 => right_c = next.pop(),
                0 => {
                    new_left.extend_from_slice(&left[k..]);
                    right_c = None;
                }
                _ => {
                    let (rest_left, rest_right) = transform_x(&left[k..], &next);
                    new_left.extend(rest_left);
                    new_right.extend(rest_right);
                    right_c = None;
                }
            }
        }
        if let Some(c) = right_c {
            new_right.push(c);
        }
        left = new_left;
    }
    (left, new_right)
}

fn is_high_surrogate(unit: u16) -> bool {
    (0xD800..0xDC00).contains(&unit)
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..0xE000).contains(&unit)
}

/// One delete and one insert covering the changed span. Enough for a save: an
/// editor buffer is replaced wholesale, and Overleaf merges what it receives.
/// The span is widened rather than split through a surrogate pair, since an op
/// carries JSON strings and half a pair is not one.
fn diff(old: &[u16], new: &[u16]) -> Vec<Component> {
    let mut prefix = old.iter().zip(new).take_while(|(a, b)| a == b).count();
    if prefix > 0 && is_high_surrogate(old[prefix - 1]) {
        prefix -= 1;
    }
    let limit = old.len().min(new.len()) - prefix;
    let mut suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(limit)
        .take_while(|(a, b)| a == b)
        .count();
    if suffix > 0 && is_low_surrogate(old[old.len() - suffix]) {
        suffix -= 1;
    }
    let mut op = Vec::new();
    let deleted = &old[prefix..old.len() - suffix];
    let inserted = &new[prefix..new.len() - suffix];
    if !deleted.is_empty() {
        op.push(Component::Delete {
            p: prefix,
            text: deleted.to_vec(),
        });
    }
    if !inserted.is_empty() {
        op.push(Component::Insert {
            p: prefix,
            text: inserted.to_vec(),
        });
    }
    op
}

/// The span of the base text an op touches, for telling two coarse diffs apart.
fn span(op: &[Component]) -> (usize, usize) {
    let mut start = usize::MAX;
    let mut end = 0;
    for c in op {
        let (p, len) = match c {
            Component::Insert { p, .. } => (*p, 0),
            Component::Delete { p, text } => (*p, text.len()),
        };
        start = start.min(p);
        end = end.max(p + len);
    }
    (start, end)
}

/// Touching counts as overlapping: two inserts at one point cannot be told
/// from one edit seen twice.
fn disjoint(a: &[Component], b: &[Component]) -> bool {
    let (a0, a1) = span(a);
    let (b0, b1) = span(b);
    a1 < b0 || b1 < a0
}

// --- documents on the wire ----------------------------------------------------

/// Overleaf sends each line with its UTF-8 bytes packed one per character, so
/// the JSON string is Latin-1 of the real text.
fn decode_lines(lines: &Value) -> Option<String> {
    let lines = lines.as_array()?;
    let mut decoded = Vec::with_capacity(lines.len());
    for line in lines {
        let line = line.as_str()?;
        let packed: Option<Vec<u8>> = line.chars().map(|c| u8::try_from(c as u32).ok()).collect();
        decoded.push(
            match packed.and_then(|bytes| String::from_utf8(bytes).ok()) {
                Some(text) => text,
                None => line.to_string(),
            },
        );
    }
    Some(decoded.join("\n"))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Folder,
    Doc,
}

struct Entity {
    parent: Option<String>,
    name: String,
    kind: Kind,
}

/// The project's file tree by id, so a doc's path survives renames and moves
/// of any folder above it.
#[derive(Default)]
struct Tree {
    entities: HashMap<String, Entity>,
}

impl Tree {
    fn from_project(project: &Value) -> Tree {
        let mut tree = Tree::default();
        if let Some(root) = project
            .get("rootFolder")
            .and_then(Value::as_array)
            .and_then(|folders| folders.first())
        {
            tree.add_folder(None, root);
        }
        tree
    }

    fn add_folder(&mut self, parent: Option<&str>, folder: &Value) {
        let Some(id) = folder.get("_id").and_then(Value::as_str) else {
            return;
        };
        self.entities.insert(
            id.to_string(),
            Entity {
                parent: parent.map(str::to_string),
                name: folder
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string(),
                kind: Kind::Folder,
            },
        );
        for doc in folder
            .get("docs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.add_doc(id, doc);
        }
        for child in folder
            .get("folders")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            self.add_folder(Some(id), child);
        }
    }

    fn add_doc(&mut self, parent: &str, doc: &Value) {
        let (Some(id), Some(name)) = (
            doc.get("_id").and_then(Value::as_str),
            doc.get("name").and_then(Value::as_str),
        ) else {
            return;
        };
        self.entities.insert(
            id.to_string(),
            Entity {
                parent: Some(parent.to_string()),
                name: name.to_string(),
                kind: Kind::Doc,
            },
        );
    }

    /// Project-relative path; `None` for the root folder and anything detached
    /// from it. Bounded by the tree's size: the parents come from the server.
    fn path_of(&self, id: &str) -> Option<String> {
        let mut parts = Vec::new();
        let mut current = id;
        for _ in 0..=self.entities.len() {
            let entity = self.entities.get(current)?;
            let Some(parent) = entity.parent.as_deref() else {
                parts.reverse();
                return (!parts.is_empty()).then(|| parts.join("/"));
            };
            parts.push(entity.name.as_str());
            current = parent;
        }
        None
    }

    fn rename(&mut self, id: &str, name: &str) {
        if let Some(entity) = self.entities.get_mut(id) {
            entity.name = name.to_string();
        }
    }

    fn relocate(&mut self, id: &str, parent: &str) {
        if let Some(entity) = self.entities.get_mut(id) {
            entity.parent = Some(parent.to_string());
        }
    }

    fn remove(&mut self, id: &str) {
        self.entities.remove(id);
    }

    fn docs(&self) -> Vec<(String, String)> {
        let mut docs: Vec<(String, String)> = self
            .entities
            .iter()
            .filter(|(_, entity)| entity.kind == Kind::Doc)
            .filter_map(|(id, _)| self.path_of(id).map(|path| (id.clone(), path)))
            .collect();
        docs.sort();
        docs
    }
}

// --- status -------------------------------------------------------------------

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Connecting,
    Live,
    Stopped,
}

#[derive(Clone, Debug)]
pub struct Status {
    pub state: State,
    /// Why the channel stopped and will not open again by itself.
    pub error: Option<String>,
    /// The cookie is what it stopped over, so a fresh one is the way back in
    /// rather than another attempt with the same.
    pub needs_session: bool,
    /// Something the user should know that is not a failure: read-only access,
    /// documents this client had to leave to the git sync.
    pub note: Option<String>,
}

impl Status {
    fn bare(state: State) -> Status {
        Status {
            state,
            error: None,
            needs_session: false,
            note: None,
        }
    }

    pub fn json(&self) -> Value {
        json!({
            "state": match self.state {
                State::Connecting => "connecting",
                State::Live => "live",
                State::Stopped => "stopped",
            },
            "error": self.error,
            "needsSession": self.needs_session,
            "note": self.note,
        })
    }
}

const BAD_COOKIE: &str = "Overleaf did not accept the session cookie.";

// --- one live session ---------------------------------------------------------

/// Where events about this session go: the dashboard tab that asked for it
/// identifies its paper by these.
#[derive(Clone, Debug)]
pub struct Scope {
    pub project_id: String,
    pub session_id: Option<String>,
    /// The paper's folder relative to the checkout, for the paths in events.
    pub folder: Option<String>,
}

pub struct Config {
    pub project: Project,
    /// The paper's folder — the Overleaf project's root, on disk. Canonical,
    /// as `resolve_project_tex` hands it out; the write guard compares paths.
    pub dir: PathBuf,
    pub cookie: String,
    pub scope: Scope,
}

/// Why a document is being left alone until the git sync settles it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Hold {
    /// Changed here and on Overleaf in the same place while no channel was open.
    Conflict,
    /// Changed here, but this account cannot write to the project.
    ReadOnly,
    /// The file on disk is not UTF-8 text, which an op cannot carry.
    Encoding,
    /// The write was refused (a symlink in the way, say); reported once.
    Unwritable,
}

type Stamp = (SystemTime, u64);

struct Doc {
    path: String,
    /// The document as Overleaf has it, plus our own edits not yet echoed.
    text: Text,
    /// The document as Overleaf has it at `version`, without our unechoed
    /// edits: the ancestor a later three-way comparison is made against.
    confirmed: Text,
    version: u64,
    /// Sent, not yet echoed by the server.
    inflight: Option<(Vec<Component>, Instant)>,
    /// Local edits made while one op was in flight, sent as the next.
    pending: Vec<Component>,
    /// Server ops applied to `text` since the file was last read or written:
    /// what a local save must be transformed through before it is sent.
    since_disk: Vec<Component>,
    /// What the file on disk looked like when last read, so a tick is a stat
    /// rather than a read. `None` forces a read.
    disk: Option<Stamp>,
    /// The file's content when last read or written.
    disk_text: Option<Text>,
    /// The file on disk uses CRLF, so a write restores it.
    crlf: bool,
    /// `text` changed since the file was last written.
    dirty: bool,
    /// A join is in flight; ops arriving meanwhile are kept for after it.
    joining: bool,
    buffered: Vec<Value>,
    held: Option<Hold>,
}

enum Awaiting {
    Join(String),
    Update(String),
}

struct Connection<'a> {
    config: &'a Config,
    shared: &'a Shared,
    tree: Tree,
    docs: HashMap<String, Doc>,
    read_only: bool,
    next_id: u64,
    awaiting: HashMap<u64, Awaiting>,
    out: Vec<String>,
    /// Files written since the dashboard was last told.
    pulled: Vec<String>,
}

enum Flushed {
    Written,
    Skipped,
    /// The file was deleted here since it was last read.
    Gone,
}

/// What ended a connection, and how soon to open the next one.
#[derive(Debug)]
enum Ended {
    Reconnect(Duration),
    /// Overleaf said something that will not change by trying again.
    Refused(String),
}

/// How many times an op for one document may be rejected before the document
/// is left to the git sync.
const MAX_REJECTIONS: u32 = 3;

impl<'a> Connection<'a> {
    fn new(config: &'a Config, shared: &'a Shared) -> Self {
        Connection {
            config,
            shared,
            tree: Tree::default(),
            docs: HashMap::new(),
            read_only: false,
            next_id: 0,
            awaiting: HashMap::new(),
            out: Vec::new(),
            pulled: Vec::new(),
        }
    }

    fn emit(&mut self, name: &str, args: Vec<Value>, awaiting: Awaiting) {
        self.next_id += 1;
        self.awaiting.insert(self.next_id, awaiting);
        self.out.push(event_frame(self.next_id, name, &args));
    }

    fn publish_status(&self, state: State) {
        let mut notes = Vec::new();
        if self.read_only {
            notes.push("You have read-only access to this project on Overleaf, so edits made here are not sent.".to_string());
        }
        for (path, reason) in self.shared.unsupported() {
            notes.push(format!("{path}: {reason} The git sync still covers it."));
        }
        let mut held: Vec<(&str, Hold)> = self
            .docs
            .values()
            .filter_map(|doc| doc.held.map(|hold| (doc.path.as_str(), hold)))
            .collect();
        held.sort_by_key(|(path, _)| *path);
        for (path, hold) in held {
            notes.push(match hold {
                Hold::Conflict => format!("{path} changed both here and on Overleaf while no connection was open. Use Sync now to choose which copy to keep."),
                Hold::ReadOnly => format!("{path} was edited here, and you have read-only access, so it stays local."),
                Hold::Encoding => format!("{path} is not UTF-8 text, so it is left alone."),
                Hold::Unwritable => continue,
            });
        }
        self.shared.set_status(Status {
            state,
            error: None,
            needs_session: false,
            note: (!notes.is_empty()).then(|| notes.join(" ")),
        });
    }

    fn handle_frame(&mut self, frame: &str) -> std::result::Result<(), Ended> {
        let Some(packet) = parse_packet(frame) else {
            return Ok(());
        };
        match packet {
            Packet::Heartbeat => self.out.push(HEARTBEAT_FRAME.to_string()),
            Packet::Connect | Packet::Other => {}
            Packet::Disconnect => return Err(Ended::Reconnect(Duration::from_secs(1))),
            // `7:::<reason>[+<advice>]`; reason 2 is "unauthorized", the rest
            // ("not handshaken", "transport not supported") clear on a new
            // handshake.
            Packet::Error { reason } => {
                return Err(if reason.starts_with('2') {
                    Ended::Refused("Overleaf refused the connection as unauthorized.".to_string())
                } else {
                    Ended::Reconnect(Duration::from_secs(5))
                });
            }
            Packet::Event { name, args } => self.handle_event(&name, args)?,
            Packet::Ack { id, args } => self.handle_ack(id, args)?,
        }
        Ok(())
    }

    fn handle_event(&mut self, name: &str, args: Vec<Value>) -> std::result::Result<(), Ended> {
        let arg = |i: usize| args.get(i).cloned().unwrap_or(Value::Null);
        match name {
            "joinProjectResponse" => {
                let response = arg(0);
                self.read_only = matches!(
                    response.get("permissionsLevel").and_then(Value::as_str),
                    Some("readOnly")
                );
                self.tree = Tree::from_project(response.get("project").unwrap_or(&Value::Null));
                for (id, path) in self.tree.docs() {
                    self.join_doc(&id, &path);
                }
                self.publish_status(State::Live);
            }
            "connectionRejected" => {
                let message = arg(0)
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("connection rejected")
                    .to_string();
                let code = arg(0)
                    .get("code")
                    .and_then(Value::as_str)
                    .map(str::to_string);
                return Err(match (message.as_str(), code.as_deref()) {
                    (_, Some("TooManyRequests")) => Ended::Reconnect(RATE_LIMIT_PAUSE),
                    ("retry", _) => Ended::Reconnect(Duration::from_secs(5)),
                    ("invalid session", _) => Ended::Refused(BAD_COOKIE.to_string()),
                    ("not authorized", _) => Ended::Refused(
                        "This Overleaf account cannot open the linked project.".to_string(),
                    ),
                    ("project not found", _) => {
                        Ended::Refused("Overleaf has no project with the linked id.".to_string())
                    }
                    _ => Ended::Refused(format!("Overleaf refused the connection: {message}.")),
                });
            }
            "reconnectGracefully" => {
                return Err(Ended::Reconnect(Duration::from_millis(
                    500 + rand_below(4_500),
                )));
            }
            "forceDisconnect" => return Err(Ended::Reconnect(Duration::from_secs(30))),
            "project:access:revoked" => {
                return Err(Ended::Refused(
                    "Your access to this Overleaf project was revoked.".to_string(),
                ));
            }
            "otUpdateApplied" => self.handle_update(&arg(0)),
            // The server disconnects after this; reconnecting rejoins every doc
            // from its current text, which is the recovery we want — unless the
            // same op keeps being refused.
            "otUpdateError" => {
                if let Some(doc_id) = arg(1).get("doc_id").and_then(Value::as_str) {
                    self.rejected(
                        doc_id,
                        arg(0).as_str().unwrap_or("Overleaf rejected an edit."),
                    );
                }
                return Err(Ended::Reconnect(Duration::from_secs(2)));
            }
            "reciveNewDoc" => {
                if let (Some(parent), Some(id)) =
                    (arg(0).as_str(), arg(1).get("_id").and_then(Value::as_str))
                {
                    self.tree.add_doc(parent, &arg(1));
                    if let Some(path) = self.tree.path_of(id) {
                        self.join_doc(id, &path);
                    }
                    self.publish_status(State::Live);
                }
            }
            "reciveNewFolder" => {
                if let Some(parent) = arg(0).as_str() {
                    self.tree.add_folder(Some(parent), &arg(1));
                    self.join_new_docs();
                }
            }
            "reciveEntityRename" => {
                if let (Some(id), Some(new_name)) = (arg(0).as_str(), arg(1).as_str()) {
                    self.tree.rename(id, new_name);
                    self.repath();
                }
            }
            "reciveEntityMove" => {
                if let (Some(id), Some(folder)) = (arg(0).as_str(), arg(1).as_str()) {
                    self.tree.relocate(id, folder);
                    self.repath();
                }
            }
            "removeEntity" => {
                // Deletions are not propagated, as in the git sync: the file
                // stays on disk, it just stops following Overleaf.
                if let Some(id) = arg(0).as_str() {
                    self.tree.remove(id);
                    self.repath();
                    self.publish_status(State::Live);
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// Join, or rejoin from Overleaf's current text. What the file on disk
    /// holds is compared against it on arrival, so anything the server sent
    /// meanwhile reaches disk first rather than being read back as a local edit.
    fn join_doc(&mut self, id: &str, path: &str) {
        if !text_doc(path)
            || confined_path(&self.config.dir, path).is_none()
            || self.shared.is_unsupported(path)
        {
            return;
        }
        if let Some(doc) = self.docs.get_mut(id) {
            // A file never read cannot be told from one edited meanwhile; the
            // join's three-way comparison sorts that out instead.
            let flushable = doc.dirty
                && doc.held.is_none()
                && doc.disk_text.is_some()
                && !self.shared.paused.load(Ordering::SeqCst);
            if flushable {
                match Self::flush_doc(self.config, self.shared, doc) {
                    Flushed::Written => self.pulled.push(doc.path.clone()),
                    Flushed::Gone => {
                        self.docs.remove(id);
                        return;
                    }
                    Flushed::Skipped => {}
                }
            }
            doc.joining = true;
            doc.inflight = None;
            doc.pending.clear();
        } else {
            self.docs.insert(
                id.to_string(),
                Doc {
                    path: path.to_string(),
                    text: Text::new(),
                    confirmed: Text::new(),
                    version: 0,
                    inflight: None,
                    pending: Vec::new(),
                    since_disk: Vec::new(),
                    disk: None,
                    disk_text: None,
                    crlf: false,
                    dirty: false,
                    joining: true,
                    buffered: Vec::new(),
                    held: None,
                },
            );
        }
        self.emit(
            "joinDoc",
            vec![json!(id), json!({ "encodeRanges": true })],
            Awaiting::Join(id.to_string()),
        );
    }

    fn join_new_docs(&mut self) {
        for (id, path) in self.tree.docs() {
            if !self.docs.contains_key(&id) {
                self.join_doc(&id, &path);
            }
        }
    }

    fn rejoin_all(&mut self) {
        let docs: Vec<(String, String)> = self
            .docs
            .iter()
            .map(|(id, doc)| (id.clone(), doc.path.clone()))
            .collect();
        for (id, path) in docs {
            self.join_doc(&id, &path);
        }
    }

    /// After a rename or move the doc ids are unchanged but their paths are
    /// not. A doc that moved out of reach (or into a name the sync would not
    /// write) is dropped; one that moved into reach is joined.
    fn repath(&mut self) {
        let paths: HashMap<String, String> = self.tree.docs().into_iter().collect();
        let mut dropped = Vec::new();
        for (id, doc) in &mut self.docs {
            match paths.get(id) {
                Some(path)
                    if text_doc(path)
                        && confined_path(&self.config.dir, path).is_some()
                        && !self.shared.is_unsupported(path) =>
                {
                    if doc.path != *path {
                        doc.path = path.clone();
                        doc.disk = None;
                        doc.disk_text = None;
                        doc.dirty = true;
                        // A file already there is somebody's; it is not replaced.
                        if let Some(Ok((existing, ..))) = read_disk(&self.config.dir, path) {
                            if existing != doc.text {
                                doc.held = Some(Hold::Conflict);
                            }
                        }
                    }
                }
                _ => dropped.push(id.clone()),
            }
        }
        for id in dropped {
            self.docs.remove(&id);
        }
        self.join_new_docs();
    }

    fn handle_ack(&mut self, id: u64, args: Vec<Value>) -> std::result::Result<(), Ended> {
        let Some(awaiting) = self.awaiting.remove(&id) else {
            return Ok(());
        };
        let error = args.first().filter(|e| !e.is_null()).map(|e| {
            e.get("message")
                .and_then(Value::as_str)
                .map(str::to_string)
                .unwrap_or_else(|| e.to_string())
        });
        match awaiting {
            Awaiting::Join(doc_id) => {
                // A doc this client cannot follow — Overleaf's newer history
                // format, or any other refusal — is one doc, not the project.
                let history_ot = args.get(5).and_then(Value::as_str) == Some("history-ot");
                let text = match (error, args.get(1).and_then(decode_lines)) {
                    (None, Some(text)) if !history_ot => text,
                    (error, _) => {
                        let reason = match error {
                            Some(error) if !error.contains("history-ot") => {
                                format!("could not be opened live ({error}).")
                            }
                            _ => {
                                "is stored in a format this client cannot follow live.".to_string()
                            }
                        };
                        if let Some(doc) = self.docs.remove(&doc_id) {
                            self.shared.mark_unsupported(&doc.path, reason);
                        }
                        self.publish_status(State::Live);
                        return Ok(());
                    }
                };
                let version = args.get(2).and_then(Value::as_u64).unwrap_or(0);
                self.joined(&doc_id, to_text(&text), version);
                // Ops that arrived while the join was out: those the snapshot
                // already holds are skipped by their version, the rest applied.
                let buffered = self
                    .docs
                    .get_mut(&doc_id)
                    .map(|doc| std::mem::take(&mut doc.buffered))
                    .unwrap_or_default();
                for update in &buffered {
                    self.handle_update(update);
                }
            }
            // A clean ack only says the op was queued; the echo confirms it.
            // The server disconnects after a rejection.
            Awaiting::Update(doc_id) => {
                if let Some(error) = error {
                    self.rejected(&doc_id, &error);
                    return Err(Ended::Reconnect(Duration::from_secs(2)));
                }
            }
        }
        Ok(())
    }

    fn rejected(&mut self, doc_id: &str, error: &str) {
        let Some(doc) = self.docs.get(doc_id) else {
            return;
        };
        if self.shared.rejection(&doc.path) >= MAX_REJECTIONS {
            self.shared.mark_unsupported(
                &doc.path,
                format!("keeps being refused by Overleaf ({error})."),
            );
        }
    }

    /// The document as Overleaf has it, against the last state both sides
    /// agreed on. Three copies decide the direction: the file moved alone
    /// (send it), Overleaf moved alone (write it), both in different places
    /// (merge), both in the same place (hold, for the git sync's conflict
    /// prompt). With no agreed state to compare against, a difference is a
    /// hold too: guessing a direction is how somebody's text gets deleted.
    fn joined(&mut self, doc_id: &str, server: Text, version: u64) {
        let read_only = self.read_only;
        let (config, shared) = (self.config, self.shared);
        let Some(doc) = self.docs.get_mut(doc_id) else {
            return;
        };
        doc.text = server.clone();
        doc.confirmed = server;
        doc.version = version;
        doc.joining = false;
        doc.inflight = None;
        doc.pending.clear();
        doc.since_disk.clear();
        doc.dirty = false;
        doc.held = None;
        // The git sync owns the files for now; `resume` joins again.
        if shared.paused.load(Ordering::SeqCst) {
            doc.disk = None;
            doc.disk_text = None;
            return;
        }
        let base = shared.base(&doc.path);
        match read_disk(&config.dir, &doc.path) {
            // Gone from disk. Deleted here, if it was ever here — that is
            // not propagated, as in the git sync, and not undone either.
            None => {
                if shared.known(&doc.path) {
                    self.docs.remove(doc_id);
                    self.publish_status(State::Live);
                    return;
                }
                doc.dirty = true;
            }
            Some(Err(())) => doc.held = Some(Hold::Encoding),
            Some(Ok((on_disk, stamp, crlf))) => {
                doc.disk = Some(stamp);
                doc.disk_text = Some(on_disk.clone());
                doc.crlf = crlf;
                if on_disk == doc.text {
                    Self::remember(shared, doc);
                } else if read_only {
                    doc.held = Some(Hold::ReadOnly);
                } else if let Some(base) = base {
                    let local = diff(&base, &on_disk);
                    // What the server has beyond the file. When that stays
                    // clear of the local edit, the server already holds the
                    // edit (an echo lost on the way) and only the rest is new.
                    let beyond = diff(&on_disk, &doc.text);
                    if local.is_empty() || disjoint(&local, &beyond) {
                        doc.since_disk = beyond;
                        doc.dirty = true;
                    } else {
                        let remote = diff(&base, &doc.text);
                        if remote.is_empty() {
                            doc.text = on_disk;
                            doc.pending = local;
                            Self::remember(shared, doc);
                        } else if disjoint(&local, &remote) {
                            let (local, remote) = transform_x(&local, &remote);
                            if apply(&mut doc.text, &local).is_ok() {
                                doc.since_disk = remote;
                                doc.pending = local;
                                doc.dirty = true;
                            } else {
                                doc.held = Some(Hold::Conflict);
                            }
                        } else {
                            doc.held = Some(Hold::Conflict);
                        }
                    }
                } else {
                    doc.held = Some(Hold::Conflict);
                }
            }
        }
        self.send_pending(doc_id);
        self.publish_status(State::Live);
    }

    /// Record the agreed state once the file holds everything confirmed: the
    /// base a later join compares against, on this or any later connection.
    fn remember(shared: &Shared, doc: &Doc) {
        if !doc.dirty {
            shared.set_base(&doc.path, &doc.confirmed);
        }
    }

    fn handle_update(&mut self, update: &Value) {
        let Some(doc_id) = update.get("doc").and_then(Value::as_str) else {
            return;
        };
        let Some(version) = update.get("v").and_then(Value::as_u64) else {
            return;
        };
        let shared = self.shared;
        let Some(doc) = self.docs.get_mut(doc_id) else {
            return;
        };
        if doc.joining {
            doc.buffered.push(update.clone());
            return;
        }
        let doc_id = doc_id.to_string();
        let path = doc.path.clone();
        match update.get("op") {
            // Our own op, acknowledged as applied — or the echo of one a
            // rejoin already gave up on, which the snapshot has settled.
            None => {
                let Some((inflight, _)) = doc.inflight.take() else {
                    return;
                };
                if version != doc.version || apply(&mut doc.confirmed, &inflight).is_err() {
                    self.join_doc(&doc_id, &path);
                    return;
                }
                doc.version += 1;
                Self::remember(shared, doc);
                self.send_pending(&doc_id);
            }
            Some(op) => {
                // An op already inside the snapshot can trail the join ack.
                if version < doc.version {
                    return;
                }
                if version > doc.version {
                    doc.buffered.push(update.clone());
                    self.join_doc(&doc_id, &path);
                    return;
                }
                let Ok(mut op) = parse_op(op) else {
                    self.join_doc(&doc_id, &path);
                    return;
                };
                if apply(&mut doc.confirmed, &op).is_err() {
                    self.join_doc(&doc_id, &path);
                    return;
                }
                if let Some((inflight, sent)) = doc.inflight.take() {
                    let (inflight, transformed) = transform_x(&inflight, &op);
                    doc.inflight = Some((inflight, sent));
                    op = transformed;
                }
                if !doc.pending.is_empty() {
                    let (pending, transformed) = transform_x(&doc.pending, &op);
                    doc.pending = pending;
                    op = transformed;
                }
                if apply(&mut doc.text, &op).is_err() {
                    self.join_doc(&doc_id, &path);
                    return;
                }
                doc.version += 1;
                doc.since_disk.extend(op);
                doc.dirty = true;
            }
        }
    }

    fn send_pending(&mut self, doc_id: &str) {
        if self.read_only {
            return;
        }
        let Some(doc) = self.docs.get_mut(doc_id) else {
            return;
        };
        if doc.inflight.is_some() || doc.pending.is_empty() || doc.joining || doc.held.is_some() {
            return;
        }
        let op = std::mem::take(&mut doc.pending);
        let payload = json!({ "doc": doc_id, "op": op_json(&op), "v": doc.version });
        doc.inflight = Some((op, Instant::now()));
        self.emit(
            "applyOtUpdate",
            vec![json!(doc_id), payload],
            Awaiting::Update(doc_id.to_string()),
        );
    }

    /// The checkout side: a file that changed since it was last read or
    /// written becomes an op, transformed through whatever the server sent
    /// since, so a save landing between two server ops merges with them.
    fn poll_local(&mut self, force: bool) {
        let config = self.config;
        let read_only = self.read_only;
        let ids: Vec<String> = self.docs.keys().cloned().collect();
        let mut changed = false;
        for id in ids {
            let Some(doc) = self.docs.get_mut(&id) else {
                continue;
            };
            if doc.joining || doc.held.is_some() {
                continue;
            }
            let Some(full) = confined_path(&config.dir, &doc.path) else {
                continue;
            };
            let stamp = std::fs::metadata(&full).ok().map(|m| stamp_of(&m));
            if stamp.is_none() || (!force && stamp == doc.disk) {
                continue;
            }
            let on_disk = match read_disk(&config.dir, &doc.path) {
                Some(Ok((on_disk, stamp, crlf))) => {
                    doc.crlf = crlf;
                    // Still being written: the next tick will see the whole file.
                    if std::fs::metadata(&full).ok().map(|m| stamp_of(&m)) != Some(stamp) {
                        continue;
                    }
                    doc.disk = Some(stamp);
                    on_disk
                }
                Some(Err(())) => {
                    doc.held = Some(Hold::Encoding);
                    changed = true;
                    continue;
                }
                None => continue,
            };
            let Some(previous) = doc.disk_text.replace(on_disk.clone()) else {
                continue;
            };
            if on_disk == previous {
                continue;
            }
            let mut local = diff(&previous, &on_disk);
            if read_only {
                doc.held = Some(Hold::ReadOnly);
                changed = true;
                continue;
            }
            if !doc.since_disk.is_empty() {
                let (transformed, since) = transform_x(&local, &doc.since_disk);
                local = transformed;
                doc.since_disk = since;
            }
            if apply(&mut doc.text, &local).is_err() {
                doc.held = Some(Hold::Conflict);
                changed = true;
                continue;
            }
            doc.pending.extend(local);
            self.send_pending(&id);
        }
        if changed {
            self.publish_status(State::Live);
        }
    }

    /// Server-side changes reach disk here. Only bytes that differ are
    /// written, so an echo of our own edit never trips the editor's
    /// changed-on-disk warning; a save not yet read is left for the poll to
    /// merge first.
    fn flush_doc(config: &Config, shared: &Shared, doc: &mut Doc) -> Flushed {
        let Some(full) = confined_path(&config.dir, &doc.path) else {
            return Flushed::Skipped;
        };
        match read_disk(&config.dir, &doc.path) {
            Some(Ok((on_disk, _, _))) => {
                if on_disk == doc.text {
                    doc.dirty = false;
                    doc.disk_text = Some(on_disk);
                    doc.since_disk.clear();
                    Self::remember(shared, doc);
                    return Flushed::Skipped;
                }
                if doc
                    .disk_text
                    .as_ref()
                    .is_some_and(|known| *known != on_disk)
                {
                    return Flushed::Skipped;
                }
            }
            // Deleted here since it was read: not undone, as in `joined`.
            None if doc.disk_text.is_some() => return Flushed::Gone,
            None => {}
            Some(Err(())) => {
                doc.dirty = false;
                doc.held = Some(Hold::Encoding);
                return Flushed::Skipped;
            }
        }
        doc.dirty = false;
        let text = from_text(&doc.text);
        let bytes = if doc.crlf {
            text.replace('\n', "\r\n")
        } else {
            text
        };
        if let Err(e) = write_pulled(&config.dir, &full, bytes.as_bytes()) {
            shared.mark_unsupported(&doc.path, format!("could not be written ({e})."));
            doc.held = Some(Hold::Unwritable);
            return Flushed::Skipped;
        }
        doc.disk_text = Some(doc.text.clone());
        doc.since_disk.clear();
        // Read rather than stamped: a save racing this write must be seen.
        doc.disk = None;
        Self::remember(shared, doc);
        Flushed::Written
    }

    fn flush(&mut self) {
        let mut gone = Vec::new();
        for (id, doc) in &mut self.docs {
            if !doc.dirty || doc.joining || doc.held.is_some() {
                continue;
            }
            match Self::flush_doc(self.config, self.shared, doc) {
                Flushed::Written => self.pulled.push(doc.path.clone()),
                Flushed::Gone => gone.push(id.clone()),
                Flushed::Skipped => {}
            }
        }
        for id in gone {
            self.docs.remove(&id);
        }
        self.emit_pulled();
    }

    fn emit_pulled(&mut self) {
        if self.pulled.is_empty() {
            return;
        }
        let paths = to_checkout(
            self.config.scope.folder.as_deref(),
            &std::mem::take(&mut self.pulled),
        );
        emit_event(
            "overleaf.pulled",
            json!({
                "key": self.shared.key,
                "projectId": self.config.scope.project_id,
                "sessionId": self.config.scope.session_id,
                "paths": paths,
            }),
        );
    }

    fn expire_inflight(&mut self) {
        let stale: Vec<(String, String)> = self
            .docs
            .iter()
            .filter(|(_, doc)| {
                doc.inflight
                    .as_ref()
                    .is_some_and(|(_, sent)| sent.elapsed() > INFLIGHT_TIMEOUT)
            })
            .map(|(id, doc)| (id.clone(), doc.path.clone()))
            .collect();
        for (id, path) in stale {
            self.join_doc(&id, &path);
        }
    }
}

fn stamp_of(metadata: &std::fs::Metadata) -> Stamp {
    (
        metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
        metadata.len(),
    )
}

/// The file as text: `None` when it is not there, `Err` when it is not UTF-8.
/// Line endings are folded to `\n` because Overleaf's document model has no
/// `\r`; sending one would leave this copy a code unit ahead of the server's.
fn read_disk(dir: &Path, rel: &str) -> Option<std::result::Result<(Text, Stamp, bool), ()>> {
    let full = confined_path(dir, rel)?;
    // `write_pulled` refuses a symlink; reading one would push whatever it
    // points at, outside the paper, into the Overleaf document.
    if std::fs::symlink_metadata(&full).is_ok_and(|m| m.file_type().is_symlink()) {
        return Some(Err(()));
    }
    let metadata = std::fs::metadata(&full).ok()?;
    let bytes = std::fs::read(&full).ok()?;
    Some(decode(bytes).map(|(text, crlf)| (text, stamp_of(&metadata), crlf)))
}

/// Overleaf's model has no `\r`, so the file is compared as LF; whether it
/// came as CRLF is remembered so a write can put it back.
fn decode(bytes: Vec<u8>) -> std::result::Result<(Text, bool), ()> {
    let text = String::from_utf8(bytes).map_err(|_| ())?;
    let crlf = text.contains("\r\n");
    Ok((
        to_text(&text.replace("\r\n", "\n").replace('\r', "\n")),
        crlf,
    ))
}

/// A little jitter without a crate for it: the clock's low bits.
fn rand_below(n: u64) -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.subsec_nanos() as u64 % n)
        .unwrap_or(0)
}

/// One handshake, one socket, until something ends it.
async fn run_connection(config: &Config, shared: &Shared) -> Ended {
    let host = config.project.live_host();
    let origin = format!("https://{host}");
    let now = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let handshake_url = format!(
        "{origin}/socket.io/1/?projectId={}&t={now}",
        config.project.id
    );
    let client = match reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .build()
    {
        Ok(client) => client,
        Err(e) => return Ended::Refused(e.to_string()),
    };
    let response = match client
        .get(&handshake_url)
        .header(reqwest::header::COOKIE, &config.cookie)
        .header(reqwest::header::ORIGIN, &origin)
        .send()
        .await
    {
        Ok(response) => response,
        Err(_) => return Ended::Reconnect(RETRY_AFTER_NETWORK_ERROR),
    };
    // The load balancer pins the socket to the pod that answered the
    // handshake through a cookie of its own, which must ride along; a
    // refreshed session cookie replaces the pasted one by name.
    let mut cookies: Vec<(String, String)> = config
        .cookie
        .split("; ")
        .filter_map(|pair| pair.split_once('='))
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    for set in response.headers().get_all(reqwest::header::SET_COOKIE) {
        let Some((name, value)) = set
            .to_str()
            .ok()
            .and_then(|s| s.split(';').next())
            .and_then(|pair| pair.trim().split_once('='))
        else {
            continue;
        };
        cookies.retain(|(existing, _)| existing != name);
        cookies.push((name.to_string(), value.to_string()));
    }
    let cookie = cookies
        .iter()
        .map(|(name, value)| format!("{name}={value}"))
        .collect::<Vec<_>>()
        .join("; ");
    let status = response.status();
    let body = response.text().await.unwrap_or_default();
    if !status.is_success() {
        return if status.as_u16() == 403 || status.as_u16() == 401 {
            Ended::Refused(BAD_COOKIE.to_string())
        } else {
            Ended::Reconnect(Duration::from_secs(10))
        };
    }
    // An expired cookie is redirected to the sign-in page, which reqwest
    // follows: a 200 with HTML where `sid:heartbeat:timeout:transports` should be.
    let Some(sid) = body
        .split(':')
        .next()
        .filter(|sid| !sid.is_empty() && !sid.contains('<'))
    else {
        return Ended::Refused(if body.trim_start().starts_with('<') {
            BAD_COOKIE.to_string()
        } else {
            "Overleaf's handshake reply was not understood.".to_string()
        });
    };

    let ws_url = format!(
        "wss://{host}/socket.io/1/websocket/{sid}?projectId={}",
        config.project.id
    );
    let mut request = match ws_url.as_str().into_client_request() {
        Ok(request) => request,
        Err(e) => return Ended::Refused(e.to_string()),
    };
    let headers = request.headers_mut();
    match (
        HeaderValue::from_str(&cookie),
        HeaderValue::from_str(&origin),
    ) {
        (Ok(cookie), Ok(origin)) => {
            headers.insert("Cookie", cookie);
            headers.insert("Origin", origin);
        }
        _ => return Ended::Refused("The session cookie is not a valid header value.".to_string()),
    }
    let socket = match tokio_tungstenite::connect_async(request).await {
        Ok((socket, _)) => socket,
        Err(_) => return Ended::Reconnect(RETRY_AFTER_NETWORK_ERROR),
    };
    let (mut sink, mut stream) = socket.split();

    let mut connection = Connection::new(config, shared);
    let mut tick = tokio::time::interval(TICK);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let ended = 'session: loop {
        let step = tokio::select! {
            message = stream.next() => {
                let _writing = lock(&shared.writing);
                match message {
                    None | Some(Ok(Message::Close(_))) => Err(Ended::Reconnect(Duration::from_secs(2))),
                    Some(Ok(Message::Text(text))) => connection.handle_frame(&text),
                    Some(Ok(_)) => Ok(()),
                    Some(Err(_)) => Err(Ended::Reconnect(RETRY_AFTER_NETWORK_ERROR)),
                }
            }
            _ = shared.nudge.notified() => {
                let _writing = lock(&shared.writing);
                if shared.rejoin.swap(false, Ordering::SeqCst) {
                    connection.rejoin_all();
                } else if !shared.paused.load(Ordering::SeqCst) {
                    connection.poll_local(true);
                }
                Ok(())
            }
            _ = tick.tick() => {
                let _writing = lock(&shared.writing);
                if shared.rejoin.swap(false, Ordering::SeqCst) {
                    connection.rejoin_all();
                } else if !shared.paused.load(Ordering::SeqCst) {
                    connection.poll_local(false);
                    connection.expire_inflight();
                    connection.flush();
                }
                Ok(())
            }
        };
        if let Err(ended) = step {
            break ended;
        }
        connection.emit_pulled();
        for frame in connection.out.drain(..) {
            if sink.send(Message::Text(frame.into())).await.is_err() {
                break 'session Ended::Reconnect(Duration::from_secs(2));
            }
        }
    };
    // Whatever the server sent reaches disk before the gap: the next join
    // reads the file back as the base, not as an edit.
    let _writing = lock(&shared.writing);
    if !shared.paused.load(Ordering::SeqCst) {
        connection.flush();
    }
    ended
}

/// Reconnects until stopped, doubling the wait after each failure; a
/// connection that lasted a while resets the doubling.
async fn run(config: Config, shared: Arc<Shared>) {
    let mut backoff = Duration::from_secs(2);
    loop {
        let started = Instant::now();
        let wait = match run_connection(&config, &shared).await {
            Ended::Refused(message) => {
                shared.set_status(Status {
                    state: State::Stopped,
                    needs_session: message == BAD_COOKIE,
                    error: Some(message),
                    note: None,
                });
                return;
            }
            Ended::Reconnect(wait) => {
                if started.elapsed() > Duration::from_secs(60) {
                    backoff = Duration::from_secs(2);
                }
                let wait = wait.max(backoff);
                backoff = (backoff * 2).min(MAX_BACKOFF);
                wait
            }
        };
        shared.set_status(Status::bare(State::Connecting));
        tokio::time::sleep(wait).await;
    }
}

// --- the sessions this process holds -----------------------------------------

/// What outlives one socket: the status the dashboard reads, and the flags
/// and locks the git sync uses to hold the channel still.
struct Shared {
    key: String,
    dir: PathBuf,
    status: Mutex<Status>,
    nudge: Notify,
    /// A git sync is writing the folder; leave the files alone until it is done.
    paused: AtomicBool,
    /// Set after a git sync so every doc is compared afresh.
    rejoin: AtomicBool,
    /// Held while the connection reads or writes files, so `pause` returns
    /// only once a step in progress is done.
    writing: Mutex<()>,
    rejections: Mutex<HashMap<String, u32>>,
    unsupported: Mutex<Vec<(String, String)>>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|e| e.into_inner())
}

/// Path to agreed text; `None` is a file both sides know to be gone from here.
type Agreed = HashMap<String, Option<Text>>;

/// Per paper folder, what both sides last agreed each file held. Kept apart
/// from the sessions: a session closed by the reaper, a cookie change, or a
/// retry must not forget what a reconnect needs to tell a local edit from a
/// remote one. Only a finished git sync replaces it.
static BASES: LazyLock<Mutex<HashMap<PathBuf, Agreed>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

impl Shared {
    fn new(key: String, dir: PathBuf) -> Shared {
        Shared {
            key,
            dir,
            status: Mutex::new(Status::bare(State::Connecting)),
            nudge: Notify::new(),
            paused: AtomicBool::new(false),
            rejoin: AtomicBool::new(false),
            writing: Mutex::new(()),
            rejections: Mutex::new(HashMap::new()),
            unsupported: Mutex::new(Vec::new()),
        }
    }

    fn status(&self) -> Status {
        lock(&self.status).clone()
    }

    fn set_status(&self, status: Status) {
        *lock(&self.status) = status.clone();
        emit_event(
            "overleaf.live",
            json!({ "key": self.key, "status": status.json() }),
        );
    }

    fn base(&self, path: &str) -> Option<Text> {
        lock(&BASES).get(&self.dir)?.get(path).cloned().flatten()
    }

    /// Whether anything was ever agreed about this path, its removal included.
    fn known(&self, path: &str) -> bool {
        lock(&BASES)
            .get(&self.dir)
            .is_some_and(|bases| bases.contains_key(path))
    }

    fn set_base(&self, path: &str, text: &[u16]) {
        lock(&BASES)
            .entry(self.dir.clone())
            .or_default()
            .insert(path.to_string(), Some(text.to_vec()));
    }

    fn rejection(&self, path: &str) -> u32 {
        let mut rejections = lock(&self.rejections);
        let count = rejections.entry(path.to_string()).or_default();
        *count += 1;
        *count
    }

    fn mark_unsupported(&self, path: &str, reason: String) {
        let mut unsupported = lock(&self.unsupported);
        if !unsupported.iter().any(|(existing, _)| existing == path) {
            unsupported.push((path.to_string(), reason));
        }
    }

    fn is_unsupported(&self, path: &str) -> bool {
        lock(&self.unsupported)
            .iter()
            .any(|(existing, _)| existing == path)
    }

    fn unsupported(&self) -> Vec<(String, String)> {
        lock(&self.unsupported).clone()
    }

    /// A retry is a fresh start for every document too.
    fn forgive(&self) {
        lock(&self.rejections).clear();
        lock(&self.unsupported).clear();
    }
}

struct Entry {
    shared: Arc<Shared>,
    task: tokio::task::JoinHandle<()>,
    last_seen: Instant,
}

impl Entry {
    /// A refusal stays on the status — it is the one line the user needs —
    /// unless what it refused has been replaced.
    fn stop(&self, keep_error: bool) {
        self.task.abort();
        if !keep_error || self.shared.status().error.is_none() {
            self.shared.set_status(Status::bare(State::Stopped));
        }
    }
}

static SESSIONS: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));
static REAPER: Once = Once::new();

type EventSink = Box<dyn Fn(&'static str, Value) + Send + Sync>;
static EVENT_SINK: OnceLock<EventSink> = OnceLock::new();

/// Where `overleaf.live` and `overleaf.pulled` events go — the dashboard's
/// event stream, once `orx up` has one.
pub fn set_event_sink(sink: EventSink) {
    let _ = EVENT_SINK.set(sink);
}

fn emit_event(name: &'static str, data: Value) {
    if let Some(sink) = EVENT_SINK.get() {
        sink(name, data);
    }
}

fn sessions() -> std::sync::MutexGuard<'static, HashMap<String, Entry>> {
    lock(&SESSIONS)
}

/// One session per paper folder and Overleaf project, whichever tab asked:
/// two sockets writing the same files would fight over them. Two dashboard
/// windows on one paper therefore share it, and whichever closes first stops
/// it for the other until its next heartbeat.
pub fn key_for(project: &Project, dir: &Path, scope: &Scope) -> String {
    format!(
        "{}:{}:{}:{}",
        scope.project_id,
        scope.session_id.as_deref().unwrap_or(""),
        project.id,
        dir.display()
    )
}

/// Open the channel, or keep it open: calling again for a running session is
/// the heartbeat that stops the reaper from closing it. One that Overleaf
/// refused stays refused until `retry` — every heartbeat is not a new
/// handshake with a bad cookie.
pub fn start(config: Config, retry: bool) -> (String, Status) {
    let key = key_for(&config.project, &config.dir, &config.scope);
    let mut live = sessions();
    let shared = match live.get_mut(&key) {
        Some(entry) => {
            entry.last_seen = Instant::now();
            let status = entry.shared.status();
            if !entry.task.is_finished() || (!retry && status.error.is_some()) {
                return (key, status);
            }
            entry.shared.forgive();
            entry.shared.set_status(Status::bare(State::Connecting));
            entry.task = tokio::spawn(run(config, entry.shared.clone()));
            entry.shared.clone()
        }
        None => {
            let shared = Arc::new(Shared::new(key.clone(), config.dir.clone()));
            let task = tokio::spawn(run(config, shared.clone()));
            live.insert(
                key.clone(),
                Entry {
                    shared: shared.clone(),
                    task,
                    last_seen: Instant::now(),
                },
            );
            shared
        }
    };
    REAPER.call_once(|| {
        tokio::spawn(async {
            loop {
                tokio::time::sleep(Duration::from_secs(30)).await;
                sessions().retain(|_, entry| {
                    let keep = entry.last_seen.elapsed() < IDLE_TIMEOUT;
                    if !keep {
                        entry.stop(true);
                    }
                    keep
                });
            }
        });
    });
    (key, shared.status())
}

pub fn stop(key: &str) {
    if let Some(entry) = sessions().remove(key) {
        entry.stop(true);
    }
}

/// The cookie changed: every tab is free to ask again with the new one.
pub fn stop_all() {
    for (_, entry) in sessions().drain() {
        entry.stop(false);
    }
}

/// The paper was unlinked or relinked: nothing may keep writing its folder,
/// and what was agreed with the old project says nothing about the next.
pub fn stop_dir(dir: &Path) {
    sessions().retain(|_, entry| {
        if entry.shared.dir == dir {
            entry.stop(true);
        }
        entry.shared.dir != dir
    });
    lock(&BASES).remove(dir);
}

/// A git sync is about to read and write this folder. Until the guard is
/// dropped, the live sessions covering it neither write files nor read them as
/// edits; afterwards every doc is compared afresh, which is how a resolved
/// conflict is released.
pub fn pause(dir: &Path) -> Paused {
    let sessions = sessions();
    let shared: Vec<Arc<Shared>> = sessions
        .values()
        .filter(|entry| entry.shared.dir == dir)
        .map(|entry| entry.shared.clone())
        .collect();
    drop(sessions);
    for shared in &shared {
        shared.paused.store(true, Ordering::SeqCst);
        // Waits out a step already reading or writing the folder.
        drop(lock(&shared.writing));
    }
    Paused {
        dir: dir.to_path_buf(),
        shared,
    }
}

/// Lets the folder go again, whether the sync finished or unwound.
pub struct Paused {
    dir: PathBuf,
    shared: Vec<Arc<Shared>>,
}

impl Paused {
    /// The agreement the sync reached, which the next join compares against.
    pub fn synced(&self, baseline: &Baseline) {
        synced(&self.dir, baseline);
    }
}

impl Drop for Paused {
    fn drop(&mut self) {
        for shared in &self.shared {
            shared.rejoin.store(true, Ordering::SeqCst);
            shared.paused.store(false, Ordering::SeqCst);
            shared.nudge.notify_one();
        }
    }
}

/// A git sync just finished, and its agreement — path to the hash of what
/// both sides hold — is where the live channel starts from. A file whose
/// bytes no longer hash to that (edited since, or never reconciled) gets no
/// base, so a join holds it rather than guessing; one the agreement names
/// but the folder lacks was deleted here, and stays so.
pub fn synced(dir: &Path, baseline: &Baseline) {
    let mut agreed = Agreed::new();
    for (path, expected) in baseline {
        if !text_doc(path) {
            continue;
        }
        let Some(full) = confined_path(dir, path) else {
            continue;
        };
        match std::fs::read(&full) {
            Ok(bytes) if hash(&bytes) == *expected => {
                if let Ok((text, _)) = decode(bytes) {
                    agreed.insert(path.clone(), Some(text));
                }
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                agreed.insert(path.clone(), None);
            }
            Err(_) => {}
        }
    }
    lock(&BASES).insert(dir.to_path_buf(), agreed);
}

/// A file was just written through the dashboard; sessions covering it can
/// look now rather than at the next tick.
pub fn nudge(path: &Path) {
    for entry in sessions().values() {
        if path.starts_with(&entry.shared.dir) {
            entry.shared.nudge.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ins(p: usize, s: &str) -> Component {
        Component::Insert {
            p,
            text: to_text(s),
        }
    }

    fn del(p: usize, s: &str) -> Component {
        Component::Delete {
            p,
            text: to_text(s),
        }
    }

    fn applied(base: &str, op: &[Component]) -> String {
        let mut text = to_text(base);
        apply(&mut text, op).unwrap();
        from_text(&text)
    }

    #[test]
    fn session_cookie_is_normalized() {
        assert_eq!(
            normalize_session("  s%3Aabc.def ").unwrap(),
            "overleaf_session2=s%3Aabc.def"
        );
        assert_eq!(
            normalize_session("Cookie: overleaf_session2=x; GCLB=y").unwrap(),
            "overleaf_session2=x; GCLB=y"
        );
        assert_eq!(
            normalize_session("sharelatex.sid=z").unwrap(),
            "sharelatex.sid=z"
        );
        assert!(normalize_session("").is_err());
        assert!(normalize_session("two words").is_err());
        assert!(normalize_session("a\nb").is_err());
    }

    #[test]
    fn packets_parse() {
        assert_eq!(parse_packet("1::"), Some(Packet::Connect));
        assert_eq!(parse_packet("2::"), Some(Packet::Heartbeat));
        assert_eq!(parse_packet("0::"), Some(Packet::Disconnect));
        assert_eq!(
            parse_packet(r#"5:::{"name":"otUpdateApplied","args":[{"doc":"d","v":3}]}"#),
            Some(Packet::Event {
                name: "otUpdateApplied".to_string(),
                args: vec![json!({"doc": "d", "v": 3})],
            })
        );
        assert_eq!(
            parse_packet("6:::4"),
            Some(Packet::Ack {
                id: 4,
                args: vec![]
            })
        );
        assert_eq!(
            parse_packet(r#"6:::7+[null,["a","b"],12]"#),
            Some(Packet::Ack {
                id: 7,
                args: vec![Value::Null, json!(["a", "b"]), json!(12)]
            })
        );
        assert_eq!(
            parse_packet("7:::1+0"),
            Some(Packet::Error {
                reason: "1+0".to_string()
            })
        );
        assert_eq!(
            event_frame(3, "joinDoc", &[json!("id"), json!({"encodeRanges": true})]),
            r#"5:3+::{"args":["id",{"encodeRanges":true}],"name":"joinDoc"}"#
        );
    }

    #[test]
    fn lines_are_unpacked_from_latin1() {
        // "é" is C3 A9 in UTF-8, packed as the two chars U+00C3 U+00A9.
        let lines = json!(["caf\u{c3}\u{a9}", "x"]);
        assert_eq!(decode_lines(&lines).unwrap(), "café\nx");
        assert_eq!(decode_lines(&json!([])).unwrap(), "");
        assert!(decode_lines(&json!({"content": "raw"})).is_none());
    }

    #[test]
    fn ops_apply_and_refuse_mismatches() {
        assert_eq!(applied("hello", &[ins(5, " world")]), "hello world");
        assert_eq!(applied("hello world", &[del(5, " world")]), "hello");
        assert_eq!(applied("héllo", &[del(1, "é"), ins(1, "e")]), "hello");
        let mut text = to_text("abc");
        assert!(apply(&mut text, &[del(0, "x")]).is_err());
        assert!(apply(&mut text, &[ins(9, "x")]).is_err());
    }

    #[test]
    fn diff_is_minimal_and_keeps_surrogate_pairs_whole() {
        assert_eq!(diff(&to_text("abc"), &to_text("abc")), vec![]);
        assert_eq!(
            diff(&to_text("hello world"), &to_text("hello there world")),
            vec![ins(6, "there ")]
        );
        assert_eq!(
            diff(&to_text("hello there world"), &to_text("hello world")),
            vec![del(6, "there ")]
        );
        assert_eq!(
            diff(&to_text("aXb"), &to_text("aYYb")),
            vec![del(1, "X"), ins(1, "YY")]
        );
        assert_eq!(diff(&to_text("abc"), &to_text("")), vec![del(0, "abc")]);
        assert_eq!(diff(&to_text(""), &to_text("abc")), vec![ins(0, "abc")]);
        // 😀 and 😁 share a high surrogate; the op must carry whole characters.
        let op = diff(&to_text("a😀b"), &to_text("a😁b"));
        assert_eq!(op, vec![del(1, "😀"), ins(1, "😁")]);
        assert_eq!(applied("a😀b", &op), "a😁b");
        // Every diff round-trips.
        for (old, new) in [
            ("", "x"),
            ("abc", "abd"),
            ("aaa", "aa"),
            ("xyz", "zyx"),
            ("a😀", "a😀😀"),
        ] {
            assert_eq!(applied(old, &diff(&to_text(old), &to_text(new))), new);
        }
    }

    #[test]
    fn spans_tell_edits_apart() {
        assert!(disjoint(&[ins(0, "a")], &[ins(5, "b")]));
        assert!(disjoint(&[del(0, "ab")], &[ins(3, "b")]));
        assert!(!disjoint(&[del(0, "ab")], &[ins(2, "b")]));
        assert!(!disjoint(&[del(0, "abc")], &[ins(2, "b")]));
        assert!(!disjoint(&[del(1, "bc"), ins(1, "x")], &[del(2, "cd")]));
        assert!(!disjoint(&[ins(3, "x")], &[ins(3, "y")]));
    }

    /// Both orders of applying two concurrent ops must converge — the property
    /// the whole live sync rests on.
    fn converges(base: &str, ours: &[Component], theirs: &[Component]) -> String {
        let (ours_after, theirs_after) = transform_x(ours, theirs);
        let mut a = to_text(base);
        apply(&mut a, ours).unwrap();
        apply(&mut a, &theirs_after).unwrap();
        let mut b = to_text(base);
        apply(&mut b, theirs).unwrap();
        apply(&mut b, &ours_after).unwrap();
        assert_eq!(from_text(&a), from_text(&b));
        from_text(&a)
    }

    #[test]
    fn concurrent_ops_converge() {
        assert_eq!(converges("abc", &[ins(0, "X")], &[ins(3, "Y")]), "XabcY");
        // Same-position inserts: ours (left) lands first.
        assert_eq!(converges("abc", &[ins(1, "L")], &[ins(1, "R")]), "aLRbc");
        assert_eq!(
            converges("abcdef", &[del(1, "bc")], &[ins(2, "X")]),
            "aXdef"
        );
        assert_eq!(
            converges("abcdef", &[del(1, "bcd")], &[del(2, "cde")]),
            "af"
        );
        assert_eq!(converges("abcdef", &[del(0, "ab")], &[del(4, "ef")]), "cd");
        assert_eq!(
            converges("abcdef", &[del(2, "cd")], &[del(2, "cd")]),
            "abef"
        );
        assert_eq!(
            converges(
                "hello world",
                &[del(0, "hello"), ins(0, "goodbye")],
                &[ins(11, "!")]
            ),
            "goodbye world!"
        );
        assert_eq!(
            converges("abc", &[del(0, "abc"), ins(0, "xyz")], &[ins(1, "Q")]),
            "xyzQ"
        );
    }

    #[test]
    fn tree_maps_docs_to_paths_through_renames_and_moves() {
        let project = json!({
            "rootFolder": [{
                "_id": "root", "name": "rootFolder",
                "docs": [{"_id": "d1", "name": "main.tex"}],
                "fileRefs": [{"_id": "f1", "name": "fig.png"}],
                "folders": [{
                    "_id": "sec", "name": "sections",
                    "docs": [{"_id": "d2", "name": "intro.tex"}],
                    "fileRefs": [], "folders": []
                }]
            }]
        });
        let mut tree = Tree::from_project(&project);
        assert_eq!(
            tree.docs(),
            vec![
                ("d1".to_string(), "main.tex".to_string()),
                ("d2".to_string(), "sections/intro.tex".to_string())
            ]
        );
        assert_eq!(tree.path_of("root"), None);
        tree.rename("sec", "chapters");
        assert_eq!(tree.path_of("d2").as_deref(), Some("chapters/intro.tex"));
        tree.relocate("d2", "root");
        assert_eq!(tree.path_of("d2").as_deref(), Some("intro.tex"));
        tree.remove("d1");
        assert_eq!(tree.docs().len(), 1);
        assert_eq!(tree.path_of("orphan"), None);
    }

    struct Paper {
        _temp: crate::local::git::TemporaryDirectory,
        dir: PathBuf,
        config: Config,
        shared: Shared,
    }

    impl Paper {
        fn new() -> Paper {
            let temp = crate::local::git::TemporaryDirectory::new("orx-live").unwrap();
            let dir = temp.path().join("paper");
            std::fs::create_dir_all(&dir).unwrap();
            let dir = crate::paths::canonicalize(&dir).unwrap();
            let config = Config {
                project: Project {
                    id: "p".to_string(),
                    host: "www.overleaf.com".to_string(),
                },
                dir: dir.clone(),
                cookie: String::new(),
                scope: Scope {
                    project_id: "orx".to_string(),
                    session_id: None,
                    folder: Some("paper".to_string()),
                },
            };
            Paper {
                _temp: temp,
                shared: Shared::new(dir.display().to_string(), dir.clone()),
                dir,
                config,
            }
        }

        fn write(&self, name: &str, text: &str) {
            std::fs::write(self.dir.join(name), text).unwrap();
        }

        fn read(&self, name: &str) -> String {
            std::fs::read_to_string(self.dir.join(name)).unwrap()
        }

        fn connect(&self) -> Connection<'_> {
            Connection::new(&self.config, &self.shared)
        }
    }

    fn join_response(docs: &[(&str, &str)]) -> Value {
        json!({
            "permissionsLevel": "owner",
            "project": {"rootFolder": [{
                "_id": "root", "name": "rootFolder",
                "docs": docs.iter().map(|(id, name)| json!({"_id": id, "name": name})).collect::<Vec<_>>(),
                "fileRefs": [], "folders": []
            }]}
        })
    }

    fn joined_ack(text: &str, version: u64) -> Vec<Value> {
        vec![Value::Null, json!([text]), json!(version)]
    }

    #[test]
    fn a_remote_edit_reaches_disk_and_a_local_save_becomes_an_op() {
        let paper = Paper::new();
        paper.write("main.tex", "hello");
        let mut c = paper.connect();

        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex"), ("n", "notes.md")])],
        )
        .unwrap();
        // Only the .tex was joined; notes.md is not a paper file.
        assert_eq!(c.out.len(), 1);
        assert!(c.out[0].contains("joinDoc"));
        c.out.clear();
        c.handle_ack(1, joined_ack("hello", 4)).unwrap();
        assert_eq!(c.docs["d1"].version, 4);
        assert!(c.out.is_empty(), "disk matches Overleaf: nothing to send");

        // Someone types on Overleaf.
        c.handle_update(&json!({"doc": "d1", "v": 4, "op": [{"p": 5, "i": " world"}]}));
        c.flush();
        assert_eq!(paper.read("main.tex"), "hello world");
        assert_eq!(c.docs["d1"].version, 5);

        // A save here: the file changes on disk, the diff goes out at v5.
        // (Sizes differ at every step, so the stamp changes without a clock.)
        paper.write("main.tex", "hello big world");
        c.poll_local(false);
        assert_eq!(c.out.len(), 1);
        let frame = c.out.remove(0);
        assert!(frame.contains(r#""applyOtUpdate""#), "{frame}");
        assert!(frame.contains(r#"{"i":"big ","p":6}"#), "{frame}");
        assert!(frame.contains(r#""v":5"#), "{frame}");

        // A second save while that op is in flight waits as pending...
        paper.write("main.tex", "hello big world!");
        c.poll_local(false);
        assert!(c.out.is_empty());
        assert_eq!(c.docs["d1"].pending, vec![ins(15, "!")]);

        // ...and a concurrent remote insert at the front shifts both.
        c.handle_update(&json!({"doc": "d1", "v": 5, "op": [{"p": 0, "i": ">"}]}));
        assert_eq!(from_text(&c.docs["d1"].text), ">hello big world!");
        assert_eq!(c.docs["d1"].pending, vec![ins(16, "!")]);
        assert_eq!(
            c.docs["d1"].inflight.as_ref().unwrap().0,
            vec![ins(7, "big ")]
        );

        // The echo of our op releases the pending one at the new version.
        c.handle_update(&json!({"doc": "d1", "v": 6}));
        assert_eq!(c.docs["d1"].version, 7);
        assert_eq!(c.out.len(), 1);
        assert!(c.out[0].contains(r#""v":7"#));
        c.flush();
        assert_eq!(paper.read("main.tex"), ">hello big world!");
    }

    #[test]
    fn a_save_before_the_flush_merges_with_the_unflushed_remote_op() {
        let paper = Paper::new();
        paper.write("main.tex", "one two");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack("one two", 1)).unwrap();
        // Remote appends before the tick writes it; the user saves a change at
        // the front meanwhile.
        c.handle_update(&json!({"doc": "d1", "v": 1, "op": [{"p": 7, "i": " three"}]}));
        paper.write("main.tex", "ONE! two");
        // The flush holds off until that save has been read.
        c.flush();
        assert_eq!(paper.read("main.tex"), "ONE! two");
        c.poll_local(false);
        assert_eq!(from_text(&c.docs["d1"].text), "ONE! two three");
        assert_eq!(
            c.docs["d1"].inflight.as_ref().unwrap().0,
            vec![del(0, "one"), ins(0, "ONE!")]
        );
        c.flush();
        assert_eq!(paper.read("main.tex"), "ONE! two three");
    }

    #[test]
    fn a_reconnect_keeps_remote_edits_made_during_the_gap() {
        let paper = Paper::new();
        paper.write("main.tex", "draft");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.handle_ack(1, joined_ack("draft", 1)).unwrap();
        drop(c);

        // A collaborator appended while we were away; nothing changed here.
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack("draft, extended", 9)).unwrap();
        assert!(c.out.is_empty(), "the remote edit must not be reverted");
        c.flush();
        assert_eq!(paper.read("main.tex"), "draft, extended");

        // Both moved, in different places: merged, and only our part is sent.
        drop(c);
        paper.write("main.tex", "Draft, extended");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack("draft, extended!", 12)).unwrap();
        assert_eq!(from_text(&c.docs["d1"].text), "Draft, extended!");
        assert_eq!(c.out.len(), 1);
        assert!(c.out[0].contains(r#"{"d":"d","p":0}"#), "{}", c.out[0]);
        assert!(!c.out[0].contains("!"), "{}", c.out[0]);
        c.flush();
        assert_eq!(paper.read("main.tex"), "Draft, extended!");

        // Both moved in the same place (we replaced the "!" they deleted):
        // held for the git sync, nothing sent and nothing overwritten.
        drop(c);
        paper.write("main.tex", "Draft, extended?");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack("Draft, extended", 14)).unwrap();
        assert!(c.out.is_empty());
        assert_eq!(c.docs["d1"].held, Some(Hold::Conflict));
        c.flush();
        assert_eq!(paper.read("main.tex"), "Draft, extended?");
        assert!(paper.shared.status().note.unwrap().contains("Sync now"));

        // The git sync settles it (say, take Overleaf's); the rejoin releases.
        paper.write("main.tex", "Draft, extended");
        c.rejoin_all();
        c.handle_ack(2, joined_ack("Draft, extended", 14)).unwrap();
        assert_eq!(c.docs["d1"].held, None);
        assert!(paper.shared.status().note.is_none());
    }

    #[test]
    fn a_version_gap_rejoins_and_a_missing_file_is_written() {
        let paper = Paper::new();
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "refs.bib")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack("@article{a}", 1)).unwrap();
        c.flush();
        assert_eq!(paper.read("refs.bib"), "@article{a}");

        // An op the snapshot already held is ignored; a gap ahead rejoins.
        c.handle_update(&json!({"doc": "d1", "v": 0, "op": [{"p": 0, "i": "x"}]}));
        assert!(!c.docs["d1"].joining);
        c.handle_update(&json!({"doc": "d1", "v": 9, "op": [{"p": 0, "i": "x"}]}));
        assert!(c.docs["d1"].joining);
        assert!(c.out.iter().any(|f| f.contains("joinDoc")));
        // Ops arriving during the rejoin wait for it rather than landing twice.
        c.handle_update(&json!({"doc": "d1", "v": 10, "op": [{"p": 0, "i": "y"}]}));
        assert_eq!(from_text(&c.docs["d1"].text), "@article{a}");
    }

    #[test]
    fn unsupported_docs_are_left_to_git_and_read_only_never_writes_over_an_edit() {
        let paper = Paper::new();
        paper.write("main.tex", "local");
        let mut c = paper.connect();
        // Joins go out in id order: d1 is old.tex, d2 is main.tex, d3 is new.tex.
        let mut response =
            join_response(&[("d1", "old.tex"), ("d2", "main.tex"), ("d3", "new.tex")]);
        response["permissionsLevel"] = json!("readOnly");
        c.handle_event("joinProjectResponse", vec![response])
            .unwrap();
        c.out.clear();
        c.handle_ack(
            1,
            vec![json!({"message": "client does not support history-ot"})],
        )
        .unwrap();
        paper.shared.set_base("main.tex", &to_text("remote"));
        c.handle_ack(2, joined_ack("remote", 1)).unwrap();
        c.handle_ack(
            3,
            vec![
                Value::Null,
                json!({"content": "raw"}),
                json!(1),
                json!([]),
                json!({}),
                json!("history-ot"),
            ],
        )
        .unwrap();
        assert!(c.out.is_empty(), "read-only: nothing is sent");
        assert_eq!(c.docs["d2"].held, Some(Hold::ReadOnly));
        c.flush();
        assert_eq!(paper.read("main.tex"), "local");
        let note = paper.shared.status().note.unwrap();
        assert!(note.contains("read-only access"), "{note}");
        assert!(note.contains("old.tex"), "{note}");
        assert!(note.contains("new.tex"), "{note}");
        assert!(note.contains("main.tex was edited here"), "{note}");
        assert_eq!(c.docs.len(), 1);
    }

    #[test]
    fn without_an_agreed_base_a_difference_is_held_not_sent() {
        let paper = Paper::new();
        paper.write("main.tex", "draft");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack("draft, extended", 3)).unwrap();
        assert!(c.out.is_empty(), "the server's lead must not be deleted");
        assert_eq!(c.docs["d1"].held, Some(Hold::Conflict));
        c.flush();
        assert_eq!(paper.read("main.tex"), "draft");

        // A git sync settles it and records what both sides now hold; from
        // then on the server's lead is written, not fought.
        paper.write("main.tex", "draft, extended");
        synced(&paper.dir, &agreement(&paper, &["main.tex"]));
        c.rejoin_all();
        c.out.clear();
        c.handle_ack(2, joined_ack("draft, extended, more", 5))
            .unwrap();
        assert_eq!(c.docs["d1"].held, None);
        assert!(c.out.is_empty());
        c.flush();
        assert_eq!(paper.read("main.tex"), "draft, extended, more");
    }

    /// The git sync's agreement for files as they are on disk now.
    fn agreement(paper: &Paper, paths: &[&str]) -> Baseline {
        paths
            .iter()
            .map(|path| {
                let bytes = std::fs::read(paper.dir.join(path)).unwrap_or_default();
                (path.to_string(), hash(&bytes))
            })
            .collect()
    }

    #[test]
    fn synced_trusts_only_what_the_git_sync_agreed() {
        let paper = Paper::new();
        paper.write("main.tex", "a");
        paper.write("refs.bib", "b");
        paper.write("scratch.tex", "edited since");
        std::fs::create_dir_all(paper.dir.join("figs")).unwrap();
        paper.write("figs/plot.png", "png");
        let mut baseline = agreement(
            &paper,
            &["main.tex", "refs.bib", "figs/plot.png", "gone.tex"],
        );
        // A file the sync agreed on earlier and left alone: the hash is old.
        baseline.insert("scratch.tex".to_string(), hash(b"original"));
        synced(&paper.dir, &baseline);
        assert_eq!(paper.shared.base("main.tex"), Some(to_text("a")));
        assert_eq!(paper.shared.base("refs.bib"), Some(to_text("b")));
        assert_eq!(paper.shared.base("scratch.tex"), None);
        assert!(!paper.shared.known("scratch.tex"));
        assert_eq!(paper.shared.base("figs/plot.png"), None);
        assert!(paper.shared.known("gone.tex"), "a deletion is remembered");
        assert_eq!(paper.shared.base("gone.tex"), None);

        // Joining the deleted file drops it rather than bringing it back.
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("g", "gone.tex")])],
        )
        .unwrap();
        c.handle_ack(1, joined_ack("old section", 1)).unwrap();
        assert!(!c.docs.contains_key("g"));
        assert!(!paper.dir.join("gone.tex").exists());
    }

    #[test]
    fn a_save_whose_echo_never_came_is_sent_again_after_a_rejoin() {
        let paper = Paper::new();
        paper.write("main.tex", "draft");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.handle_ack(1, joined_ack("draft", 1)).unwrap();
        paper.write("main.tex", "draft!");
        c.poll_local(false);
        c.out.clear();
        assert!(c.docs["d1"].inflight.is_some());

        // The socket drops before the echo; the next connection joins with
        // the server still at the old text, and a collaborator's insert at
        // the front arrives alongside.
        drop(c);
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack(">draft", 2)).unwrap();
        assert_eq!(c.out.len(), 1, "the unechoed edit is sent again");
        assert!(c.out[0].contains(r#"{"i":"!","p":6}"#), "{}", c.out[0]);
        assert_eq!(from_text(&c.docs["d1"].text), ">draft!");
        c.flush();
        assert_eq!(paper.read("main.tex"), ">draft!");

        // The echo of an op a rejoin gave up on is not a reason to rejoin.
        c.docs.get_mut("d1").unwrap().inflight = None;
        c.handle_update(&json!({"doc": "d1", "v": 2}));
        assert!(!c.docs["d1"].joining);
    }

    #[test]
    fn an_edit_the_server_already_applied_is_not_sent_twice() {
        let paper = Paper::new();
        paper.write("main.tex", "draft");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.handle_ack(1, joined_ack("draft", 1)).unwrap();
        paper.write("main.tex", "draft!");
        c.poll_local(false);
        drop(c);

        // The server applied it (echo lost) and a collaborator prepended.
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack(">draft!", 3)).unwrap();
        assert!(c.out.is_empty(), "nothing to send: {:?}", c.out);
        assert_eq!(c.docs["d1"].held, None);
        c.flush();
        assert_eq!(paper.read("main.tex"), ">draft!");

        // A second local save queued behind it is not merged into a
        // duplicate either: same place, so it is held for the user.
        drop(c);
        paper.write("main.tex", ">draft!?");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack(">draft!!", 4)).unwrap();
        assert!(c.out.is_empty());
        assert_eq!(c.docs["d1"].held, Some(Hold::Conflict));
    }

    #[test]
    fn a_file_deleted_here_stays_deleted_and_a_relink_forgets_the_agreement() {
        let paper = Paper::new();
        paper.write("main.tex", "draft");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.handle_ack(1, joined_ack("draft", 1)).unwrap();
        assert!(paper.shared.base("main.tex").is_some());
        std::fs::remove_file(paper.dir.join("main.tex")).unwrap();
        c.rejoin_all();
        c.handle_ack(2, joined_ack("draft", 1)).unwrap();
        assert!(!c.docs.contains_key("d1"));
        assert!(!paper.dir.join("main.tex").exists());

        stop_dir(&paper.dir);
        assert!(paper.shared.base("main.tex").is_none());
    }

    #[test]
    fn ops_arriving_during_a_rejoin_are_kept_and_replayed() {
        let paper = Paper::new();
        paper.write("main.tex", "ab");
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex")])],
        )
        .unwrap();
        c.handle_ack(1, joined_ack("ab", 1)).unwrap();
        // A gap: v3 when v1 was expected.
        c.handle_update(&json!({"doc": "d1", "v": 3, "op": [{"p": 3, "i": "d"}]}));
        assert!(c.docs["d1"].joining);
        // The snapshot arrives at v3 (holding v1 and v2); the buffered v3 op
        // is applied after it, and one from before the snapshot is skipped.
        c.handle_update(&json!({"doc": "d1", "v": 2, "op": [{"p": 0, "i": "x"}]}));
        c.handle_ack(2, joined_ack("abc", 3)).unwrap();
        assert_eq!(from_text(&c.docs["d1"].text), "abcd");
        assert_eq!(c.docs["d1"].version, 4);
        assert!(!c.docs["d1"].joining);
    }

    #[test]
    fn crlf_and_non_utf8_files() {
        let paper = Paper::new();
        paper.write("main.tex", "a\r\nb");
        std::fs::write(paper.dir.join("refs.bib"), b"caf\xe9").unwrap();
        let mut c = paper.connect();
        c.handle_event(
            "joinProjectResponse",
            vec![join_response(&[("d1", "main.tex"), ("d2", "refs.bib")])],
        )
        .unwrap();
        c.out.clear();
        c.handle_ack(1, joined_ack("a\nb", 1)).unwrap();
        c.handle_ack(2, joined_ack("cafe", 1)).unwrap();
        assert!(c.out.is_empty(), "CRLF is not a difference");
        assert_eq!(c.docs["d2"].held, Some(Hold::Encoding));
        c.flush();
        assert_eq!(
            paper.read("main.tex"),
            "a\r\nb",
            "a file that matches is not rewritten"
        );
    }
}
