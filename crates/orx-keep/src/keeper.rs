//! The keeper process: one child, its retained output, and at most one attached server.

use crate::platform;
use crate::wire::{
    self, kind, Control, Endpoint, Exit, Hello, Refused, Retain, Signal, Spec, Welcome,
};
use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// The whole handshake, not each read: a sender that drips bytes must not hold a slot.
const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// Connections allowed to be mid-handshake; more are dropped so strangers cannot exhaust threads.
const MAX_HANDSHAKES: usize = 32;
/// How long output the child's descendants still write may follow its exit.
const EOF_GRACE: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(250);
/// How long after the child's exit a stop still kills its group: later, the group id may name
/// a recycled process.
const LEFTOVER_WINDOW: Duration = Duration::from_secs(30);
const CURSOR_QUERY: &[u8] = b"\x1b[6n";

pub(crate) struct Spawned {
    pub pid: u32,
    pub input: Option<Box<dyn Write + Send>>,
    pub output: Box<dyn Read + Send>,
    pub process: Box<dyn Process>,
    pub wait: Box<dyn FnOnce() -> Exit + Send>,
    pub answers_cursor_queries: bool,
    /// Whether an `interrupt` is typed into the terminal as `\x03` rather than signalled.
    pub types_interrupt: bool,
}

pub(crate) trait Process: Send + Sync {
    fn resize(&self, cols: u16, rows: u16);
    fn signal(&self, signal: Signal);
    /// Ends the child and everything it started that the platform can attribute to it.
    fn kill_tree(&self);
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis() as u64)
        .unwrap_or_default()
}

/// `orx keep DIR` and `orx-host keep DIR`: read the spec from stdin, start the child, print
/// `ready` (or `error: …`) on stdout, then serve attachments until the child's exit is delivered.
pub fn run(dir: &Path) -> io::Result<()> {
    let mut input = Vec::new();
    io::stdin().read_to_end(&mut input)?;
    let spec = match wire::parse::<Spec>(&input) {
        Ok(spec) if spec.protocol == wire::PROTOCOL => spec,
        Ok(spec) => {
            return refuse(io::Error::other(format!(
                "this keeper speaks protocol {}, not {}",
                wire::PROTOCOL,
                spec.protocol
            )))
        }
        Err(error) => return refuse(error),
    };
    if let Err(error) = platform::detach() {
        return refuse(error);
    }
    let started = match start(dir, &spec) {
        Ok(started) => started,
        Err(error) => return refuse(error),
    };
    println!("ready");
    let _ = io::stdout().flush();
    platform::silence_stdout();
    serve(dir, spec, started)
}

fn refuse(error: io::Error) -> io::Result<()> {
    println!("error: {error}");
    let _ = io::stdout().flush();
    Err(error)
}

struct Started {
    _lock: File,
    listener: TcpListener,
    spawned: Spawned,
}

fn start(dir: &Path, spec: &Spec) -> io::Result<Started> {
    let lock = OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(dir.join(wire::LOCK_FILE))?;
    lock.try_lock()
        .map_err(|_| io::Error::other("another keeper already holds this directory"))?;
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0))?;
    let spawned = platform::spawn(spec)
        .map_err(|error| io::Error::new(error.kind(), format!("{}: {error}", spec.program)))?;
    publish(
        &dir.join(wire::ENDPOINT_FILE),
        &wire::json(&Endpoint {
            protocol: wire::PROTOCOL,
            port: listener.local_addr()?.port(),
            token: spec.token.clone(),
            keeper_pid: std::process::id(),
            child_pid: spawned.pid,
        }),
    )?;
    Ok(Started {
        _lock: lock,
        listener,
        spawned,
    })
}

/// Readers see a whole file or none.
fn publish(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let temporary = path.with_extension("part");
    let mut file = File::create(&temporary)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    fs::rename(&temporary, path)
}

struct State {
    retained: VecDeque<u8>,
    oldest: u64,
    eof: bool,
    exit: Option<(Exit, Instant)>,
    published: Option<Instant>,
    /// The current attachment's generation; 0 when none.
    attachment: u64,
    generations: u64,
    stream: Option<TcpStream>,
    /// Whether the current attachment has been sent the exit.
    delivered: bool,
    detached_at: Instant,
    stop: bool,
    killed: Option<&'static str>,
    cursor_query: bool,
}

impl State {
    fn next(&self) -> u64 {
        self.oldest + self.retained.len() as u64
    }
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
    /// Feeds the thread writing to the child, so a child that does not read never holds up a frame.
    input: Mutex<Option<Sender<Vec<u8>>>>,
    process: Box<dyn Process>,
    retain: Retain,
    token: String,
    child_pid: u32,
    answers_cursor_queries: bool,
    types_interrupt: bool,
    handshakes: AtomicUsize,
}

impl Shared {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Queues bytes for the child, in order after those already queued.
    fn write_input(&self, bytes: Vec<u8>) {
        let mut input = self.input.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(queue) = input.as_ref() {
            // The writer ends when the child closed its stdin; dropping the queue says so.
            if queue.send(bytes).is_err() {
                *input = None;
            }
        }
    }

    fn answer_cursor_query(&self, state: &mut State) {
        if state.cursor_query {
            state.cursor_query = false;
            self.write_input(b"\x1b[1;1R".to_vec());
        }
    }
}

/// Writes queued input to the child until the queue closes or the child stops reading. The
/// queue holds whatever the one attached server sends, which the server buffers the same way.
fn write_queued(mut writer: Box<dyn Write + Send>, queue: Receiver<Vec<u8>>) {
    for bytes in queue {
        if writer
            .write_all(&bytes)
            .and_then(|()| writer.flush())
            .is_err()
        {
            return;
        }
    }
}

fn serve(dir: &Path, spec: Spec, started: Started) -> io::Result<()> {
    let Started {
        _lock,
        listener,
        spawned,
    } = started;
    let orphan = Duration::from_secs(spec.orphan_seconds);
    let input = spawned.input.map(|writer| {
        let (queue, queued) = mpsc::channel();
        std::thread::spawn(move || write_queued(writer, queued));
        queue
    });
    let shared = Arc::new(Shared {
        state: Mutex::new(State {
            retained: VecDeque::new(),
            oldest: 0,
            eof: false,
            exit: None,
            published: None,
            attachment: 0,
            generations: 0,
            stream: None,
            delivered: false,
            detached_at: Instant::now(),
            stop: false,
            killed: None,
            cursor_query: false,
        }),
        changed: Condvar::new(),
        input: Mutex::new(input),
        process: spawned.process,
        retain: spec.retain,
        token: spec.token,
        child_pid: spawned.pid,
        answers_cursor_queries: spawned.answers_cursor_queries,
        types_interrupt: spawned.types_interrupt,
        handshakes: AtomicUsize::new(0),
    });

    let reading = shared.clone();
    let mut output = spawned.output;
    std::thread::spawn(move || pump(&reading, &mut *output));
    let waiting = shared.clone();
    let wait = spawned.wait;
    std::thread::spawn(move || {
        let exit = wait();
        let mut state = waiting.lock();
        state.exit = Some((exit, Instant::now()));
        waiting.changed.notify_all();
    });
    let accepting = shared.clone();
    std::thread::spawn(move || {
        for mut stream in listener.incoming().flatten() {
            if accepting.handshakes.load(Ordering::Relaxed) >= MAX_HANDSHAKES {
                continue;
            }
            accepting.handshakes.fetch_add(1, Ordering::Relaxed);
            let shared = accepting.clone();
            let spawned = std::thread::Builder::new().spawn(move || {
                let hello = handshake(&shared, &mut stream);
                shared.handshakes.fetch_sub(1, Ordering::Relaxed);
                if let Some(hello) = hello {
                    attach(&shared, stream, hello);
                }
            });
            // A failed spawn drops the connection; the next one may still find room.
            if spawned.is_err() {
                accepting.handshakes.fetch_sub(1, Ordering::Relaxed);
            }
        }
    });

    let mut state = shared.lock();
    loop {
        let now = Instant::now();
        if state.published.is_none() {
            if let Some((exit, at)) = &state.exit {
                if state.eof || now.duration_since(*at) >= EOF_GRACE {
                    let mut exit = exit.clone();
                    exit.reason = state.killed.map(str::to_string);
                    exit.unfinished_output = !state.eof;
                    if let Err(error) = publish(&dir.join(wire::EXIT_FILE), &wire::json(&exit)) {
                        eprintln!("orx keep: could not record the exit: {error}");
                    }
                    state.exit = Some((exit, *at));
                    state.published = Some(now);
                    shared.changed.notify_all();
                }
            }
        }
        let alone = state.attachment == 0;
        let idle_since = state.published.map_or(state.detached_at, |published| {
            published.max(state.detached_at)
        });
        let orphaned = alone && now.duration_since(idle_since) >= orphan;
        if state.exit.is_none() && state.killed.is_none() && (state.stop || orphaned) {
            state.killed = Some(if state.stop { "stopped" } else { "orphaned" });
            drop(state);
            shared.process.kill_tree();
            state = shared.lock();
            continue;
        }
        if state.published.is_some() && ((state.stop && (alone || state.delivered)) || orphaned) {
            break;
        }
        state = shared
            .changed
            .wait_timeout(state, TICK)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
    drop(state);
    std::process::exit(0)
}

/// Retains the child's output under the spec's policy until it ends.
fn pump(shared: &Shared, output: &mut dyn Read) {
    let mut chunk = vec![0; wire::MAX_OUTPUT];
    loop {
        if let Retain::Unacked { limit } = shared.retain {
            let mut state = shared.lock();
            while state.retained.len() as u64 >= limit && state.killed.is_none() {
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        }
        let read = match output.read(&mut chunk) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        };
        let bytes = &chunk[..read];
        let mut state = shared.lock();
        state.retained.extend(bytes);
        if let Retain::Tail { bytes: keep } = shared.retain {
            let excess = state.retained.len().saturating_sub(keep as usize);
            state.retained.drain(..excess);
            state.oldest += excess as u64;
        }
        if shared.answers_cursor_queries && contains(bytes, CURSOR_QUERY) {
            state.cursor_query = true;
            if state.attachment == 0 {
                shared.answer_cursor_query(&mut state);
            }
        }
        shared.changed.notify_all();
    }
    let mut state = shared.lock();
    state.eof = true;
    shared.changed.notify_all();
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

/// Reads against one deadline: `set_read_timeout` alone restarts on every read.
struct Deadline<'a> {
    stream: &'a TcpStream,
    at: Instant,
}

impl Read for Deadline<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let left = self.at.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return Err(io::ErrorKind::TimedOut.into());
        }
        self.stream.set_read_timeout(Some(left))?;
        self.stream.read(buf)
    }
}

/// The valid hello a connection opens with; anyone else is dropped, or refused with a reason.
fn handshake(shared: &Shared, stream: &mut TcpStream) -> Option<Hello> {
    let mut reader = Deadline {
        stream,
        at: Instant::now() + HELLO_TIMEOUT,
    };
    let hello = match wire::read_frame(&mut reader) {
        Ok((kind::HELLO, body)) => wire::parse::<Hello>(&body).ok()?,
        _ => return None,
    };
    if hello.protocol == wire::PROTOCOL && hello.token == shared.token {
        return Some(hello);
    }
    let error = if hello.protocol != wire::PROTOCOL {
        format!(
            "this keeper speaks protocol {}, not {}",
            wire::PROTOCOL,
            hello.protocol
        )
    } else {
        "wrong token".to_string()
    };
    let _ = wire::write_frame(stream, kind::REFUSED, &wire::json(&Refused { error }));
    None
}

fn attach(shared: &Arc<Shared>, mut stream: TcpStream, hello: Hello) {
    let _ = stream.set_read_timeout(None);
    let (Ok(writer), Ok(registered)) = (stream.try_clone(), stream.try_clone()) else {
        return;
    };
    // Taken with the switch, so every cursor query before `next` has been answered.
    let (generation, welcome) = {
        let mut state = shared.lock();
        shared.answer_cursor_query(&mut state);
        state.generations += 1;
        if let Some(previous) = state.stream.replace(registered) {
            let _ = previous.shutdown(Shutdown::Both);
        }
        state.attachment = state.generations;
        state.delivered = false;
        shared.changed.notify_all();
        let welcome = Welcome {
            protocol: wire::PROTOCOL,
            child_pid: shared.child_pid,
            start: hello.from.clamp(state.oldest, state.next()),
            next: state.next(),
        };
        (state.attachment, welcome)
    };
    let sending = shared.clone();
    let sender =
        std::thread::Builder::new().spawn(move || send(&sending, writer, generation, welcome));
    if sender.is_err() {
        // Closing the connection ends the read loop below, which detaches this attachment.
        let _ = stream.shutdown(Shutdown::Both);
    }

    while let Ok((frame, body)) = wire::read_frame(&mut stream) {
        if shared.lock().attachment != generation {
            break;
        }
        match frame {
            kind::INPUT => {
                shared.lock().cursor_query = false;
                shared.write_input(body);
            }
            kind::ACK => {
                let Ok((offset, _)) = wire::split_offset(&body) else {
                    continue;
                };
                if let Retain::Unacked { .. } = shared.retain {
                    let mut state = shared.lock();
                    let through = offset.min(state.next());
                    if through > state.oldest {
                        let released = (through - state.oldest) as usize;
                        state.retained.drain(..released);
                        state.oldest = through;
                        shared.changed.notify_all();
                    }
                }
            }
            kind::CONTROL => match wire::parse::<Control>(&body) {
                Ok(Control::Resize { cols, rows }) => shared.process.resize(cols, rows),
                Ok(Control::Signal {
                    signal: Signal::Interrupt,
                }) if shared.types_interrupt => shared.write_input(b"\x03".to_vec()),
                Ok(Control::Signal { signal }) => shared.process.signal(signal),
                Ok(Control::CloseStdin) => {
                    shared
                        .input
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .take();
                }
                Ok(Control::Stop) => {
                    let mut state = shared.lock();
                    // Held across the kill: the loop must not exit before what the child left is ended.
                    if state
                        .exit
                        .as_ref()
                        .is_some_and(|(_, at)| at.elapsed() < LEFTOVER_WINDOW)
                    {
                        shared.process.signal(Signal::Kill);
                    }
                    state.stop = true;
                    shared.changed.notify_all();
                }
                Ok(Control::Unknown) | Err(_) => {}
            },
            _ => {}
        }
    }

    let mut state = shared.lock();
    if state.attachment == generation {
        state.attachment = 0;
        state.stream = None;
        state.detached_at = Instant::now();
        shared.answer_cursor_query(&mut state);
        shared.changed.notify_all();
    }
}

enum Next {
    Output(u64, Vec<u8>),
    Exit(Exit),
}

fn send(shared: &Shared, mut stream: TcpStream, generation: u64, welcome: Welcome) {
    let mut sent = welcome.start;
    if wire::write_frame(&mut stream, kind::WELCOME, &wire::json(&welcome)).is_err() {
        return;
    }
    let mut exit_sent = false;
    loop {
        let next = {
            let mut state = shared.lock();
            loop {
                if state.attachment != generation {
                    return;
                }
                // Tail retention dropped bytes this attachment had not read yet.
                sent = sent.max(state.oldest);
                if sent < state.next() {
                    let from = (sent - state.oldest) as usize;
                    let to = (from + wire::MAX_OUTPUT).min(state.retained.len());
                    let bytes = state.retained.range(from..to).copied().collect();
                    break Next::Output(sent, bytes);
                }
                if let (Some(_), Some((exit, _)), false) = (state.published, &state.exit, exit_sent)
                {
                    break Next::Exit(exit.clone());
                }
                state = shared
                    .changed
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let written = match &next {
            Next::Output(offset, bytes) => wire::write_frame(
                &mut stream,
                kind::OUTPUT,
                &wire::output_body(*offset, bytes),
            ),
            Next::Exit(exit) => wire::write_frame(&mut stream, kind::EXIT, &wire::json(exit)),
        };
        if written.is_err() {
            let _ = stream.shutdown(Shutdown::Both);
            return;
        }
        match next {
            Next::Output(_, bytes) => sent += bytes.len() as u64,
            Next::Exit(_) => {
                exit_sent = true;
                let mut state = shared.lock();
                if state.attachment == generation {
                    state.delivered = true;
                    shared.changed.notify_all();
                }
            }
        }
    }
}
