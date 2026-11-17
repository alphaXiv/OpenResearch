//! The keeper's frozen surfaces (FROZEN.md): its spec, its files and its frames.

use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL: u32 = 1;
/// Larger frames are refused rather than buffered.
pub const MAX_FRAME: usize = 16 << 20;
/// Output frames carry at most this many bytes.
pub const MAX_OUTPUT: usize = 64 << 10;

pub const LOCK_FILE: &str = "keeper.lock";
pub const ENDPOINT_FILE: &str = "endpoint.json";
pub const EXIT_FILE: &str = "exit.json";
pub const OWNER_FILE: &str = "owner.json";
pub const LOG_FILE: &str = "keeper.log";

pub mod kind {
    pub const HELLO: u8 = 0x01;
    pub const INPUT: u8 = 0x02;
    pub const ACK: u8 = 0x03;
    pub const CONTROL: u8 = 0x04;
    pub const WELCOME: u8 = 0x81;
    pub const OUTPUT: u8 = 0x82;
    pub const EXIT: u8 = 0x83;
    pub const REFUSED: u8 = 0x84;
}

/// What the keeper starts, read once from its stdin.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Spec {
    pub protocol: u32,
    /// Proves an attachment comes from the server that started the keeper or its successor.
    pub token: String,
    pub program: String,
    pub args: Vec<String>,
    /// Changes over the keeper's inherited environment; `None` removes the variable.
    pub env: Vec<(String, Option<String>)>,
    #[serde(default)]
    pub env_clear: bool,
    pub cwd: Option<String>,
    pub stdio: Stdio,
    pub retain: Retain,
    /// How long the child outlives the last attachment.
    pub orphan_seconds: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Stdio {
    /// Separate stdin and stdout pipes; stderr appends to `stderr` or is discarded.
    Pipes { stderr: Option<String> },
    /// One pseudoterminal for input and output.
    Pty { cols: u16, rows: u16 },
    /// A stdin pipe, and one pipe for stdout and stderr in the order the child writes them.
    Merged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Retain {
    /// Keep every byte not yet acknowledged; past `limit` the child waits to be read.
    Unacked { limit: u64 },
    /// Keep the newest `bytes`, dropping older ones, and never make the child wait.
    Tail { bytes: u64 },
}

/// `endpoint.json`, published once the keeper listens.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Endpoint {
    pub protocol: u32,
    pub port: u16,
    pub token: String,
    pub keeper_pid: u32,
    pub child_pid: u32,
}

/// `exit.json`, and the body of an exit frame.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Exit {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub finished_at: u64,
    /// Why the keeper ended the child, when it did: `orphaned` or `stopped`.
    #[serde(default)]
    pub reason: Option<String>,
    /// The child's output had not ended when its exit was recorded: something it started
    /// still held its stdout, and output after the exit may be missing.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub unfinished_output: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    pub protocol: u32,
    pub token: String,
    /// The first output offset wanted; the keeper starts at its oldest retained byte if later.
    pub from: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Welcome {
    pub protocol: u32,
    pub child_pid: u32,
    /// The offset output frames start at: `from`, or the oldest byte still retained if later.
    pub start: u64,
    pub next: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Refused {
    pub error: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum Control {
    Resize {
        cols: u16,
        rows: u16,
    },
    Signal {
        signal: Signal,
    },
    CloseStdin,
    /// End the child's process tree, deliver its exit, then exit.
    Stop,
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Signal {
    Interrupt,
    Terminate,
    Kill,
    #[serde(other)]
    Unknown,
}

pub fn write_frame(writer: &mut impl Write, kind: u8, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len() + 1)
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    let mut frame = Vec::with_capacity(body.len() + 5);
    frame.extend_from_slice(&len.to_le_bytes());
    frame.push(kind);
    frame.extend_from_slice(body);
    writer.write_all(&frame)
}

pub fn read_frame(reader: &mut impl Read) -> io::Result<(u8, Vec<u8>)> {
    let mut len = [0; 4];
    reader.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len) as usize;
    if len == 0 || len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame length {len} out of range"),
        ));
    }
    let mut frame = vec![0; len];
    reader.read_exact(&mut frame)?;
    let kind = frame.remove(0);
    Ok((kind, frame))
}

pub fn output_body(offset: u64, bytes: &[u8]) -> Vec<u8> {
    let mut body = Vec::with_capacity(bytes.len() + 8);
    body.extend_from_slice(&offset.to_le_bytes());
    body.extend_from_slice(bytes);
    body
}

/// An output or ack body's leading offset and the bytes after it.
pub fn split_offset(body: &[u8]) -> io::Result<(u64, &[u8])> {
    let (offset, rest) = body
        .split_first_chunk::<8>()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "frame lacks its offset"))?;
    Ok((u64::from_le_bytes(*offset), rest))
}

pub fn json<T: Serialize>(value: &T) -> Vec<u8> {
    serde_json::to_vec(value).expect("keeper wire types serialize")
}

pub fn parse<T: for<'de> Deserialize<'de>>(body: &[u8]) -> io::Result<T> {
    serde_json::from_slice(body).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_round_trip_and_reject_oversized_lengths() {
        let mut bytes = Vec::new();
        write_frame(&mut bytes, kind::OUTPUT, &output_body(7, b"hi")).unwrap();
        let (kind, body) = read_frame(&mut bytes.as_slice()).unwrap();
        assert_eq!(kind, kind::OUTPUT);
        assert_eq!(split_offset(&body).unwrap(), (7, &b"hi"[..]));

        let oversized = ((MAX_FRAME + 1) as u32).to_le_bytes();
        assert!(read_frame(&mut oversized.as_slice()).is_err());
    }

    #[test]
    fn unknown_controls_and_signals_parse_for_forward_compatibility() {
        assert_eq!(
            parse::<Control>(br#"{"op":"rename","to":"x"}"#).unwrap(),
            Control::Unknown
        );
        assert_eq!(
            parse::<Control>(br#"{"op":"signal","signal":"quit"}"#).unwrap(),
            Control::Signal {
                signal: Signal::Unknown
            }
        );
    }
}
