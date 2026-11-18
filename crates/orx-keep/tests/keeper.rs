#![allow(clippy::disallowed_methods)]

use orx_keep::wire::{self, kind, Control, Endpoint, Exit, Hello, Retain, Spec, Stdio, Welcome};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpStream;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_orx-keep");
const TOKEN: &str = "test-token";

fn scratch() -> PathBuf {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let dir = std::env::temp_dir().join(format!(
        "orx-keep-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn spec(role: &str, stdio: Stdio, retain: Retain, orphan_seconds: u64) -> Spec {
    Spec {
        protocol: wire::PROTOCOL,
        token: TOKEN.into(),
        program: BIN.into(),
        args: vec![role.into()],
        env: vec![("ORX_KEEP_TEST".into(), Some("1".into()))],
        env_clear: false,
        cwd: None,
        stdio,
        retain,
        orphan_seconds,
    }
}

fn pipes() -> Stdio {
    Stdio::Pipes { stderr: None }
}

const LOSSLESS: Retain = Retain::Unacked { limit: 1 << 20 };

/// Starts a keeper the way the server does and returns its endpoint.
fn start(dir: &Path, spec: &Spec) -> Endpoint {
    // On Windows the started process is the keeper itself and outlives this handle.
    #[cfg_attr(windows, allow(clippy::zombie_processes))]
    let mut keeper = Command::new(BIN)
        .arg("keep")
        .arg(dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::fs::File::create(dir.join(wire::LOG_FILE)).unwrap())
        .spawn()
        .unwrap();
    keeper
        .stdin
        .take()
        .unwrap()
        .write_all(&wire::json(spec))
        .unwrap();
    let mut line = String::new();
    BufReader::new(keeper.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    assert_eq!(line.trim(), "ready", "{}", log(dir));
    #[cfg(unix)]
    assert!(keeper.wait().unwrap().success());
    wire::parse(&std::fs::read(dir.join(wire::ENDPOINT_FILE)).unwrap()).unwrap()
}

fn log(dir: &Path) -> String {
    std::fs::read_to_string(dir.join(wire::LOG_FILE)).unwrap_or_default()
}

struct Link {
    stream: TcpStream,
    welcome: Welcome,
    position: u64,
}

impl Link {
    fn attach(endpoint: &Endpoint, from: u64) -> Link {
        let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(10)))
            .unwrap();
        let hello = Hello {
            protocol: wire::PROTOCOL,
            token: TOKEN.into(),
            from,
        };
        wire::write_frame(&mut stream, kind::HELLO, &wire::json(&hello)).unwrap();
        let (frame, body) = wire::read_frame(&mut stream).unwrap();
        assert_eq!(frame, kind::WELCOME);
        let welcome: Welcome = wire::parse(&body).unwrap();
        Link {
            stream,
            position: welcome.start,
            welcome,
        }
    }

    fn input(&mut self, bytes: &[u8]) {
        wire::write_frame(&mut self.stream, kind::INPUT, bytes).unwrap();
    }

    fn control(&mut self, control: &Control) {
        wire::write_frame(&mut self.stream, kind::CONTROL, &wire::json(control)).unwrap();
    }

    fn ack(&mut self, offset: u64) {
        wire::write_frame(&mut self.stream, kind::ACK, &offset.to_le_bytes()).unwrap();
    }

    /// Output from the start offset until `done` holds, checking offsets are contiguous.
    fn read_until(&mut self, done: impl Fn(&[u8]) -> bool) -> (Vec<u8>, Option<Exit>) {
        let mut output = Vec::new();
        loop {
            let (frame, body) = wire::read_frame(&mut self.stream).unwrap_or_else(|error| {
                panic!(
                    "{error}; output so far: {:?}",
                    String::from_utf8_lossy(&output)
                )
            });
            match frame {
                kind::OUTPUT => {
                    let (offset, bytes) = wire::split_offset(&body).unwrap();
                    assert_eq!(offset, self.position, "output must be contiguous");
                    self.position += bytes.len() as u64;
                    output.extend_from_slice(bytes);
                    // Answer where the cursor is, as a terminal does; ConPTY waits for it. A
                    // query before `next` is replay the keeper answered while none was attached,
                    // and a second answer reaches cmd.exe as F3.
                    if offset >= self.welcome.next && contains(bytes, "\x1b[6n") {
                        self.input(b"\x1b[1;1R");
                    }
                    if done(&output) {
                        return (output, None);
                    }
                }
                kind::EXIT => return (output, Some(wire::parse(&body).unwrap())),
                other => panic!("unexpected frame {other:#x}"),
            }
        }
    }

    fn read_exit(&mut self) -> (Vec<u8>, Exit) {
        let (output, exit) = self.read_until(|_| false);
        (output, exit.expect("the keeper delivers the exit"))
    }
}

fn contains(haystack: &[u8], needle: &str) -> bool {
    String::from_utf8_lossy(haystack).contains(needle)
}

fn wait_for(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(25));
    }
}

fn keeper_alive(dir: &Path) -> bool {
    let lock = std::fs::File::open(dir.join(wire::LOCK_FILE)).unwrap();
    lock.try_lock_shared().is_err()
}

#[test]
fn input_reaches_the_child_and_its_output_comes_back() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("cat", pipes(), LOSSLESS, 60));
    let mut link = Link::attach(&endpoint, 0);
    assert_eq!(link.welcome.start, 0);
    link.input(b"hello\n");
    let (output, _) = link.read_until(|output| contains(output, "hello\n"));
    assert_eq!(output, b"hello\n");

    link.control(&Control::CloseStdin);
    let (_, exit) = link.read_exit();
    assert_eq!(exit.code, Some(0));
    assert_eq!(exit.reason, None);
    link.control(&Control::Stop);
    wait_for("the keeper to exit", || !keeper_alive(&dir));
    let recorded: Exit = wire::parse(&std::fs::read(dir.join(wire::EXIT_FILE)).unwrap()).unwrap();
    assert_eq!(recorded, exit);
}

#[test]
fn a_child_that_never_reads_cannot_hold_up_a_stop() {
    let dir = scratch();
    let mut spec = spec("sleep", pipes(), LOSSLESS, 60);
    spec.args.push("60".into());
    let endpoint = start(&dir, &spec);
    let mut link = Link::attach(&endpoint, 0);
    // More than any pipe holds, so the keeper's write to the child blocks.
    link.input(&vec![b'x'; 1 << 20]);
    link.input(b"after");
    let stopping = Instant::now();
    link.control(&Control::Stop);
    let (_, exit) = link.read_exit();
    assert_eq!(exit.reason.as_deref(), Some("stopped"));
    assert!(stopping.elapsed() < Duration::from_secs(5));
    wait_for("the keeper to exit", || !keeper_alive(&dir));
}

#[test]
fn a_reattached_server_gets_everything_since_its_last_ack_without_gaps() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("spam", pipes(), LOSSLESS, 60));
    let mut first = Link::attach(&endpoint, 0);
    let (seen, _) = first.read_until(|output| output.len() >= 200);
    let acked = 100;
    first.ack(acked);
    // Windows resets a socket closed with unread data, which can discard a frame just sent.
    std::thread::sleep(Duration::from_millis(100));
    drop(first);

    // The child keeps writing while no server is attached.
    std::thread::sleep(Duration::from_millis(300));
    let mut second = Link::attach(&endpoint, 0);
    assert_eq!(second.welcome.start, acked);
    let (resumed, _) = second.read_until(|output| output.len() >= 2000);
    assert_eq!(
        &resumed[..seen.len() - acked as usize],
        &seen[acked as usize..]
    );
    let text = String::from_utf8_lossy(&resumed);
    let numbers: Vec<u64> = text
        .lines()
        .filter_map(|line| line.strip_prefix("line ")?.parse().ok())
        .collect();
    assert!(numbers.windows(2).all(|pair| pair[1] == pair[0] + 1));

    second.control(&Control::Stop);
    let (_, exit) = second.read_exit();
    assert_eq!(exit.reason.as_deref(), Some("stopped"));
    wait_for("the keeper to exit", || !keeper_alive(&dir));
}

#[test]
fn unacknowledged_output_holds_the_child_at_its_limit() {
    let dir = scratch();
    let limit = 512;
    let endpoint = start(&dir, &spec("spam", pipes(), Retain::Unacked { limit }, 60));
    std::thread::sleep(Duration::from_millis(500));
    let mut link = Link::attach(&endpoint, 0);
    let held = link.welcome.next;
    assert!(held >= limit && held <= limit + wire::MAX_OUTPUT as u64);
    // Acknowledging lets it go on.
    while link.position < held + limit * 2 {
        link.read_until(|_| true);
        let position = link.position;
        link.ack(position);
    }
    link.control(&Control::Stop);
    link.read_exit();
}

#[test]
fn tail_retention_keeps_the_newest_output_and_never_holds_the_child() {
    let dir = scratch();
    let endpoint = start(
        &dir,
        &spec("spam", pipes(), Retain::Tail { bytes: 256 }, 60),
    );
    std::thread::sleep(Duration::from_millis(500));
    let mut link = Link::attach(&endpoint, 0);
    assert!(link.welcome.start > 0);
    assert!(link.welcome.next - link.welcome.start <= 256);
    link.control(&Control::Stop);
    link.read_exit();
}

#[test]
fn an_orphaned_child_is_ended_and_its_exit_recorded() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("spam", pipes(), LOSSLESS, 1));
    let link = Link::attach(&endpoint, 0);
    drop(link);
    wait_for("the orphaned keeper to exit", || !keeper_alive(&dir));
    let exit: Exit = wire::parse(&std::fs::read(dir.join(wire::EXIT_FILE)).unwrap()).unwrap();
    assert_eq!(exit.reason.as_deref(), Some("orphaned"));
}

#[test]
fn a_wrong_token_is_refused() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("cat", pipes(), LOSSLESS, 60));
    let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port)).unwrap();
    let hello = Hello {
        protocol: wire::PROTOCOL,
        token: "guess".into(),
        from: 0,
    };
    wire::write_frame(&mut stream, kind::HELLO, &wire::json(&hello)).unwrap();
    let (frame, _) = wire::read_frame(&mut stream).unwrap();
    assert_eq!(frame, kind::REFUSED);
    let mut link = Link::attach(&endpoint, 0);
    link.control(&Control::Stop);
    link.read_exit();
}

/// Whether the keeper has closed `stream`, waiting up to `wait` for it to say so.
fn closed_within(stream: &mut TcpStream, wait: Duration) -> bool {
    stream.set_read_timeout(Some(wait)).unwrap();
    match stream.read(&mut [0; 1]) {
        Ok(read) => read == 0,
        Err(error) => !matches!(
            error.kind(),
            std::io::ErrorKind::WouldBlock | std::io::ErrorKind::TimedOut
        ),
    }
}

#[test]
fn strangers_cannot_hold_unlimited_handshakes_open() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("cat", pipes(), LOSSLESS, 60));
    let mut idle: Vec<TcpStream> = (0..256)
        .map(|_| TcpStream::connect(("127.0.0.1", endpoint.port)).unwrap())
        .collect();
    // The keeper waits five seconds on each hello it admits and drops the rest at once, so
    // one quick pass finishes inside that wait.
    let mut dropped = 0;
    for stream in &mut idle {
        dropped += usize::from(closed_within(stream, Duration::from_millis(10)));
    }
    assert!(
        dropped >= 256 - 64,
        "only {dropped} idle connections dropped"
    );

    drop(idle);
    wait_for("the keeper to admit a real attachment", || {
        let Ok(mut stream) = TcpStream::connect(("127.0.0.1", endpoint.port)) else {
            return false;
        };
        let hello = Hello {
            protocol: wire::PROTOCOL,
            token: TOKEN.into(),
            from: 0,
        };
        wire::write_frame(&mut stream, kind::HELLO, &wire::json(&hello)).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        matches!(wire::read_frame(&mut stream), Ok((kind::WELCOME, _)))
    });
}

#[test]
fn a_hello_dripped_slowly_is_dropped_at_the_deadline() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("cat", pipes(), LOSSLESS, 60));
    let mut stream = TcpStream::connect(("127.0.0.1", endpoint.port)).unwrap();
    // A length announcing a body that never completes, one byte every half second.
    let started = Instant::now();
    let announced = 1000u32.to_le_bytes();
    let mut sent = 0;
    while !closed_within(&mut stream, Duration::from_millis(500)) {
        assert!(started.elapsed() < Duration::from_secs(9), "never dropped");
        let _ = stream.write_all(announced.get(sent..=sent).unwrap_or(&[0]));
        sent += 1;
    }
    assert!(started.elapsed() >= Duration::from_secs(4));
}

#[test]
fn a_new_attachment_replaces_the_old_one() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("cat", pipes(), LOSSLESS, 60));
    let mut old = Link::attach(&endpoint, 0);
    let mut new = Link::attach(&endpoint, 0);
    assert!(wire::read_frame(&mut old.stream).is_err());
    new.input(b"still here\n");
    new.read_until(|output| contains(output, "still here"));
    new.control(&Control::Stop);
    new.read_exit();
}

#[cfg(unix)]
#[test]
fn a_dead_keeper_takes_its_child_with_it() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("cat", pipes(), LOSSLESS, 60));
    let alive = |pid: u32| unsafe { libc::kill(pid as i32, 0) } == 0;
    assert!(alive(endpoint.child_pid));
    unsafe {
        libc::kill(endpoint.keeper_pid as i32, libc::SIGKILL);
    }
    // The child is the keeper's: once the keeper is gone, `cat` reads end of input and exits.
    wait_for("the child to exit", || !alive(endpoint.child_pid));
}

#[cfg(unix)]
#[test]
fn a_terminal_runs_a_shell_that_sees_its_size() {
    let dir = scratch();
    let mut spec = spec("unused", Stdio::Pty { cols: 91, rows: 33 }, LOSSLESS, 60);
    spec.program = "/bin/sh".into();
    spec.args = vec![];
    let endpoint = start(&dir, &spec);
    let mut link = Link::attach(&endpoint, 0);
    link.input(b"stty size; echo marker-$((40+2))\n");
    link.read_until(|output| contains(output, "33 91") && contains(output, "marker-42"));
    link.control(&Control::Resize {
        cols: 120,
        rows: 40,
    });
    link.input(b"stty size\n");
    link.read_until(|output| contains(output, "40 120"));
    link.input(b"exit 3\n");
    let (_, exit) = link.read_exit();
    assert_eq!(exit.code, Some(3));
}

#[cfg(windows)]
#[test]
fn a_terminal_runs_a_shell() {
    let dir = scratch();
    let mut spec = spec(
        "unused",
        Stdio::Pty {
            cols: 100,
            rows: 30,
        },
        LOSSLESS,
        60,
    );
    spec.program = "cmd.exe".into();
    spec.args = vec![];
    let endpoint = start(&dir, &spec);
    let mut link = Link::attach(&endpoint, 0);
    // Enter is a carriage return, as a terminal sends it.
    link.input(b"echo marker-%PROCESSOR_ARCHITECTURE%\r");
    // The expanded line, not the echoed command; ConPTY may put escapes anywhere around it.
    link.read_until(|output| {
        String::from_utf8_lossy(output)
            .match_indices("marker-")
            .any(|(at, found)| !output[at + found.len()..].starts_with(b"%"))
    });
    link.input(b"exit 3\r");
    let (_, exit) = link.read_exit();
    assert_eq!(exit.code, Some(3));
}

#[test]
fn merged_output_keeps_the_order_the_child_wrote() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("both", Stdio::Merged, LOSSLESS, 60));
    let mut link = Link::attach(&endpoint, 0);
    let (output, exit) = link.read_exit();
    assert_eq!(exit.code, Some(0));
    assert!(!exit.unfinished_output);
    let text = String::from_utf8_lossy(&output).replace("\r\n", "\n");
    assert_eq!(text, "out 1\nerr 1\nout 2\nerr 2\n");
    link.control(&Control::Stop);
}

#[test]
fn an_exit_whose_output_never_ended_says_so() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("linger", pipes(), LOSSLESS, 60));
    let mut link = Link::attach(&endpoint, 0);
    let started = Instant::now();
    let (_, exit) = link.read_exit();
    assert_eq!(exit.code, Some(0));
    assert!(exit.unfinished_output);
    assert!(started.elapsed() < Duration::from_secs(4));
    link.control(&Control::Stop);
}

#[cfg(unix)]
#[test]
fn a_stop_reaches_descendants_outside_the_childs_group() {
    let dir = scratch();
    let endpoint = start(&dir, &spec("detach", pipes(), LOSSLESS, 60));
    let mut link = Link::attach(&endpoint, 0);
    let (output, _) = link.read_until(|output| output.ends_with(b"\n"));
    let grandchild: i32 = String::from_utf8_lossy(&output).trim().parse().unwrap();
    let alive = |pid: i32| unsafe { libc::kill(pid, 0) } == 0;
    assert!(alive(grandchild));

    link.control(&Control::Stop);
    let (_, exit) = link.read_exit();
    assert_eq!(exit.reason.as_deref(), Some("stopped"));
    // Reaped by init once killed; a zombie answers kill(0) until then.
    wait_for("the detached descendant to die", || {
        !alive(grandchild)
            || std::fs::read_to_string(format!("/proc/{grandchild}/stat"))
                .is_ok_and(|stat| stat.contains(") Z "))
    });
}
