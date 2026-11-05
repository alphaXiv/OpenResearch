use anyhow::Result;
use socket2::{Domain, Protocol, Socket, Type};
use std::collections::VecDeque;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, Shutdown, SocketAddr, TcpListener, TcpStream};
use std::num::NonZeroU64;
use std::os::fd::AsRawFd;
use std::os::unix::net::UnixStream;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

const MAX_QUEUED_CHUNKS: usize = 16;

/// Applied to both directions of every connection, including existing connections.
#[derive(Clone, Copy, Debug, Default)]
pub struct Faults {
    /// Delay each received chunk once, without blocking other connections.
    pub latency: Duration,
    /// Per-direction, per-connection bytes/second; no initial burst allowance.
    pub bytes_per_second: Option<NonZeroU64>,
    /// Read and discard traffic while leaving TCP connections open.
    pub black_hole: bool,
    /// Stop reading and writing without discarding buffered traffic.
    pub paused: bool,
    /// Close the listening socket; established connections remain alive.
    pub refuse_new: bool,
}

enum Control {
    Set(Faults, Sender<io::Result<()>>),
    Reset(Sender<io::Result<()>>),
    Stop,
}

/// One worker owns all sockets; control calls acknowledge after the change takes effect.
pub struct FaultProxy {
    port: u16,
    control: Sender<Control>,
    wake: UnixStream,
    worker: Option<JoinHandle<()>>,
}

impl FaultProxy {
    pub fn start(target: SocketAddr) -> Result<Self> {
        let listener = listen(0)?;
        let port = listener.local_addr()?.port();
        let (control, receiver) = mpsc::channel();
        let (wake, worker_wake) = UnixStream::pair()?;
        wake.set_nonblocking(true)?;
        worker_wake.set_nonblocking(true)?;
        let worker = thread::Builder::new()
            .name("sshd-fault-proxy".into())
            .spawn(move || {
                serve(listener, target, receiver, worker_wake);
            })?;
        Ok(Self {
            port,
            control,
            wake,
            worker: Some(worker),
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn set(&self, faults: Faults) -> Result<()> {
        self.update(|ack| Control::Set(faults, ack))
    }

    pub fn clear(&self) -> Result<()> {
        self.set(Faults::default())
    }

    /// Drops established connections while keeping the configured faults.
    pub fn reset(&self) -> Result<()> {
        self.update(Control::Reset)
    }

    fn update(&self, message: impl FnOnce(Sender<io::Result<()>>) -> Control) -> Result<()> {
        let (sender, receiver) = mpsc::channel();
        self.send(message(sender))?;
        receiver.recv_timeout(Duration::from_secs(2))??;
        Ok(())
    }

    fn send(&self, message: Control) -> Result<()> {
        self.control.send(message)?;
        match (&self.wake).write_all(&[1]) {
            Ok(()) => Ok(()),
            // A full wake socket already guarantees that poll will return.
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}

impl Drop for FaultProxy {
    fn drop(&mut self) {
        let _ = self.send(Control::Stop);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn listen(port: u16) -> io::Result<TcpListener> {
    let socket = Socket::new(Domain::IPV4, Type::STREAM, Some(Protocol::TCP))?;
    socket.set_reuse_address(true)?;
    socket.set_nonblocking(true)?;
    socket.bind(&SocketAddr::from((Ipv4Addr::LOCALHOST, port)).into())?;
    socket.listen(128)?;
    Ok(socket.into())
}

struct Chunk {
    ready: Instant,
    bytes: Vec<u8>,
    written: usize,
}

#[derive(Default)]
struct Direction {
    queue: VecDeque<Chunk>,
    eof: bool,
}

impl Direction {
    fn pump(&mut self, from: &mut TcpStream, to: &mut TcpStream, faults: Faults) -> io::Result<()> {
        if faults.paused {
            return Ok(());
        }
        if faults.black_hole {
            self.queue.clear();
        }
        // Bound queued memory and preserve backpressure when the receiver stalls.
        for _ in self.queue.len()..MAX_QUEUED_CHUNKS {
            if self.eof {
                break;
            }
            let mut bytes = [0; 4096];
            match from.read(&mut bytes) {
                Ok(0) => self.eof = true,
                Ok(n) if !faults.black_hole => {
                    let now = Instant::now();
                    let previous = self.queue.back().map(|chunk| chunk.ready).unwrap_or(now);
                    let transfer = faults
                        .bytes_per_second
                        .map(|rate| Duration::from_secs_f64(n as f64 / rate.get() as f64))
                        .unwrap_or_default();
                    self.queue.push_back(Chunk {
                        ready: (now + faults.latency).max(previous) + transfer,
                        bytes: bytes[..n].to_vec(),
                        written: 0,
                    });
                }
                Ok(_) => {}
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        while let Some(chunk) = self.queue.front_mut() {
            if chunk.ready > Instant::now() {
                break;
            }
            match to.write(&chunk.bytes[chunk.written..]) {
                Ok(0) => return Err(io::ErrorKind::WriteZero.into()),
                Ok(n) => {
                    chunk.written += n;
                    if chunk.written == chunk.bytes.len() {
                        self.queue.pop_front();
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                Err(e) => return Err(e),
            }
        }
        if self.eof && self.queue.is_empty() {
            to.shutdown(Shutdown::Write)?;
        }
        Ok(())
    }
}

struct Connection {
    client: TcpStream,
    server: TcpStream,
    upstream: Direction,
    downstream: Direction,
}

impl Connection {
    fn reset(&self) {
        for stream in [&self.client, &self.server] {
            let _ = socket2::SockRef::from(stream).set_linger(Some(Duration::ZERO));
        }
    }

    fn pump(&mut self, faults: Faults) -> io::Result<bool> {
        self.upstream
            .pump(&mut self.client, &mut self.server, faults)?;
        self.downstream
            .pump(&mut self.server, &mut self.client, faults)?;
        Ok(!(self.upstream.eof
            && self.downstream.eof
            && self.upstream.queue.is_empty()
            && self.downstream.queue.is_empty()))
    }
}

fn serve(
    listener: TcpListener,
    target: SocketAddr,
    receiver: Receiver<Control>,
    mut wake: UnixStream,
) {
    let port = listener.local_addr().unwrap().port();
    let mut listener = Some(listener);
    let mut faults = Faults::default();
    let mut connections: Vec<Connection> = Vec::new();
    'worker: loop {
        let mut bytes = [0; 256];
        loop {
            match wake.read(&mut bytes) {
                Ok(0) => break 'worker,
                Ok(_) => {}
                Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(_) => break 'worker,
            }
        }
        while let Ok(message) = receiver.try_recv() {
            match message {
                Control::Stop => break 'worker,
                Control::Set(next, ack) => {
                    let result = if next.refuse_new {
                        listener = None;
                        Ok(())
                    } else if listener.is_none() {
                        listen(port).map(|socket| listener = Some(socket))
                    } else {
                        Ok(())
                    };
                    if result.is_ok() {
                        faults = next;
                        if faults.black_hole {
                            for connection in &mut connections {
                                connection.upstream.queue.clear();
                                connection.downstream.queue.clear();
                            }
                        }
                    }
                    let _ = ack.send(result);
                }
                Control::Reset(ack) => {
                    for connection in &connections {
                        connection.reset();
                    }
                    connections.clear();
                    let _ = ack.send(Ok(()));
                }
            }
        }
        if let Some(listener) = &listener {
            // Bound accepts so a connection flood cannot starve control messages.
            for _ in 0..32 {
                match listener.accept() {
                    Ok((client, _)) => {
                        // The target is loopback; a short deadline bounds teardown even if sshd dies.
                        let connection = (|| -> Result<Connection> {
                            let server =
                                TcpStream::connect_timeout(&target, Duration::from_millis(50))?;
                            client.set_nonblocking(true)?;
                            server.set_nonblocking(true)?;
                            client.set_nodelay(true)?;
                            server.set_nodelay(true)?;
                            Ok(Connection {
                                client,
                                server,
                                upstream: Direction::default(),
                                downstream: Direction::default(),
                            })
                        })();
                        if let Ok(connection) = connection {
                            connections.push(connection);
                        }
                    }
                    Err(e) if e.kind() == io::ErrorKind::WouldBlock => break,
                    Err(_) => break,
                }
            }
        }
        connections.retain_mut(|connection| connection.pump(faults).unwrap_or(false));
        if wait(&wake, listener.as_ref(), &connections, faults).is_err() {
            break;
        }
    }
    for connection in &connections {
        connection.reset();
    }
}

fn wait(
    wake: &UnixStream,
    listener: Option<&TcpListener>,
    connections: &[Connection],
    faults: Faults,
) -> io::Result<()> {
    let mut sockets = vec![libc::pollfd {
        fd: wake.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    }];
    if let Some(listener) = listener {
        sockets.push(libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        });
    }
    let now = Instant::now();
    let mut deadline = None;
    if !faults.paused {
        for connection in connections {
            let client = connection.client.as_raw_fd();
            let server = connection.server.as_raw_fd();
            for (from, to, direction) in [
                (client, server, &connection.upstream),
                (server, client, &connection.downstream),
            ] {
                if !direction.eof && direction.queue.len() < MAX_QUEUED_CHUNKS {
                    sockets.push(libc::pollfd {
                        fd: from,
                        events: libc::POLLIN,
                        revents: 0,
                    });
                }
                if let Some(chunk) = direction.queue.front() {
                    if chunk.ready <= now {
                        sockets.push(libc::pollfd {
                            fd: to,
                            events: libc::POLLOUT,
                            revents: 0,
                        });
                    } else {
                        deadline =
                            Some(deadline.map_or(chunk.ready, |previous: Instant| {
                                previous.min(chunk.ready)
                            }));
                    }
                }
            }
        }
    }
    let timeout = deadline.map_or(-1, |deadline| {
        deadline
            .saturating_duration_since(Instant::now())
            .as_nanos()
            .div_ceil(1_000_000)
            .min(i32::MAX as u128) as i32
    });
    // Descriptors stay owned by this worker throughout the poll call.
    let result =
        unsafe { libc::poll(sockets.as_mut_ptr(), sockets.len() as libc::nfds_t, timeout) };
    if result < 0 {
        let error = io::Error::last_os_error();
        if error.kind() != io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
    Ok(())
}
