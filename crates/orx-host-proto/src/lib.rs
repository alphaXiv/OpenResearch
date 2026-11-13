use serde::{de::DeserializeOwned, Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u32 = 5;
pub const MAX_FRAME_LEN: usize = 8 * 1024 * 1024;
pub const MAX_PAGE_LEN: u64 = 1024 * 1024;
pub const MAX_WAIT_MS: u32 = 30_000;
pub type RunId = String;
pub type Sha = String;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Message<T> {
    pub id: u64,
    pub body: T,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Request {
    Hello {
        proto: u32,
    },
    Launch(LaunchIntent),
    Inspect {
        run: RunId,
    },
    Changes {
        watch: Vec<(RunId, Summary)>,
        follow_logs: Vec<RunId>,
        wait_ms: u32,
    },
    ReadLog {
        target: LogTarget,
        file: LogFile,
        ident: Option<FileIdent>,
        range: ByteRange,
        #[serde(
            serialize_with = "bytes::serialize_optional",
            deserialize_with = "bytes::deserialize_optional"
        )]
        anchor: Option<[u8; 64]>,
    },
    Cancel {
        run: RunId,
    },
    Cleanup {
        run: RunId,
    },
    CancelLegacy {
        dir: String,
        container: Option<ContainerRun>,
    },
    Exec {
        command: String,
        #[serde(with = "bytes")]
        stdin: Vec<u8>,
        timeout_ms: u32,
    },
    HasBlob {
        sha: Sha,
        len: u64,
    },
    /// Holds a staged blob in a run dir as `source.tar`; across filesystems each request copies
    /// a bounded slice, so the client repeats it until `pinned`.
    PinSource {
        run: RunId,
        sha: Sha,
    },
    PutBlob {
        sha: Sha,
        offset: u64,
        #[serde(with = "bytes")]
        bytes: Vec<u8>,
    },
    /// Publishes the launch like `Launch` without starting it, and submits a batch job of
    /// `directives` (`#SBATCH` lines) that starts it.
    SlurmSubmit {
        intent: LaunchIntent,
        directives: String,
    },
    SlurmQueue {
        names: Vec<RunId>,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        legacy: Vec<SlurmLegacyRun>,
    },
    SlurmCancelLegacy {
        job: String,
    },
    InspectLegacy {
        dir: String,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum Response {
    Hello {
        proto: u32,
        binary_sha: Sha,
        boot_id: String,
        os: String,
        arch: String,
        kernel: String,
    },
    Launch {
        summary: Summary,
    },
    Inspect {
        summary: Summary,
    },
    Changes {
        runs: Vec<(RunId, Summary)>,
    },
    ReadLog {
        page: LogPage,
    },
    Cancel {
        outcome: CancelAck,
    },
    Cleanup {
        outcome: CleanupAck,
    },
    CancelLegacy {
        outcome: LegacyCancelOutcome,
    },
    Exec {
        result: ExecResult,
    },
    HasBlob {
        present: bool,
    },
    PinSource {
        pinned: bool,
    },
    PutBlob {
        len: u64,
    },
    SlurmSubmit {
        job: String,
    },
    SlurmQueue {
        jobs: Vec<SlurmJob>,
    },
    InspectLegacy {
        inspection: LegacyInspection,
    },
    SlurmCancelLegacy,
    Error {
        code: ErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    InvalidRequest,
    Conflict,
    NotFound,
    Unsupported,
    Busy,
    Io,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LogFile {
    Log,
    Notify,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum LogTarget {
    Run(RunId),
    Dir(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContainerRun {
    pub reference: String,
    pub id: String,
    pub started_at: String,
    pub run_dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ByteRange {
    pub start: u64,
    pub end: u64,
}

/// A log file's inode; runs write each log once at a run-unique path. Serialized as the
/// opaque string `dev-ino`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(into = "String", try_from = "String")]
pub struct FileIdent {
    pub dev: u64,
    pub ino: u64,
}

impl From<FileIdent> for String {
    fn from(ident: FileIdent) -> Self {
        format!("{}-{}", ident.dev, ident.ino)
    }
}

impl TryFrom<String> for FileIdent {
    type Error = String;
    fn try_from(text: String) -> Result<Self, String> {
        let (dev, ino) = text.split_once('-').ok_or("invalid log identity")?;
        Ok(Self {
            dev: dev.parse().map_err(|_| "invalid log identity")?,
            ino: ino.parse().map_err(|_| "invalid log identity")?,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogInfo {
    pub ident: FileIdent,
    pub len: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LogPage {
    Data {
        len: Option<u64>,
        ident: FileIdent,
        start: u64,
        #[serde(rename = "data", with = "bytes")]
        bytes: Vec<u8>,
        eof: bool,
    },
    Reset {
        info: Option<LogInfo>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub node: String,
    pub boot_id: String,
    pub pid: u32,
    pub starttime: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Survivor {
    pub pid: u32,
    pub starttime: u64,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum CancelOutcome {
    Verified,
    Survivors,
    Unverifiable { unreadable: Vec<u32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    Exited,
    Cancelled,
    SpawnFailed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CancelAck {
    Accepted,
    NotRunning,
    WrapperGone,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CleanupAck {
    Accepted,
    NothingLingering,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LegacyCancelOutcome {
    GroupGone,
    Survivors { pids: Vec<u32> },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecResult {
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub timed_out: bool,
    #[serde(with = "bytes")]
    pub stdout: Vec<u8>,
    #[serde(with = "bytes")]
    pub stderr: Vec<u8>,
    pub stdout_truncated: bool,
    pub stderr_truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExitReceipt {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub finished_at: Option<i64>,
    pub code: Option<i32>,
    pub signal: Option<i32>,
    pub reason: ExitReason,
    pub processes: Vec<Survivor>,
    pub outcome: CancelOutcome,
    /// Why a run that never started failed to, in words for the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProcessOutcome {
    pub processes: Vec<Survivor>,
    pub outcome: CancelOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum RunState {
    Missing,
    Staging,
    /// `reason` is the scheduler's word on why a queued job has not started.
    Starting {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    Running,
    Exited {
        receipt: ExitReceipt,
    },
    Failed {
        reason: String,
    },
    Lost {
        reason: String,
    },
    Cancelled {
        reason: String,
    },
    Unreconciled {
        detail: String,
    },
    InvalidData {
        detail: String,
    },
    Unreachable {
        cause: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<i64>,
    pub state: RunState,
    pub log_len: Option<u64>,
    pub notify_len: u64,
    pub cleanup: Option<ProcessOutcome>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchIntent {
    pub run: RunId,
    pub digest: Sha,
    pub run_sh: String,
    pub env: BTreeMap<String, String>,
    pub source: Option<Sha>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cancel_sh: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlurmJob {
    pub run: RunId,
    pub job: String,
    pub state: String,
    pub summary: Summary,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SlurmLegacyRun {
    pub run: RunId,
    pub job: String,
    pub dir: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LegacyInspection {
    pub pid: Option<u32>,
    pub alive: bool,
    pub exit_code: Option<i32>,
    pub receipt_unreadable: bool,
}

/// A clean EOF is distinct from a truncated header or payload; lengths are big-endian.
pub fn read_frame<T: DeserializeOwned>(reader: &mut impl Read) -> io::Result<Option<T>> {
    let mut header = [0; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let len = u32::from_be_bytes(header) as usize;
    if len == 0 || len > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid frame length",
        ));
    }
    let mut payload = vec![0; len];
    reader.read_exact(&mut payload)?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
}

pub fn write_frame<T: Serialize>(writer: &mut impl Write, value: &T) -> io::Result<()> {
    let payload = serde_json::to_vec(value).map_err(io::Error::other)?;
    if payload.len() > MAX_FRAME_LEN {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "frame too large",
        ));
    }
    writer.write_all(&(payload.len() as u32).to_be_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()
}

mod bytes {
    use base64::{engine::general_purpose::STANDARD, Engine};
    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer, T: AsRef<[u8]>>(
        value: &T,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        STANDARD.encode(value).serialize(serializer)
    }

    pub fn deserialize<'de, D: Deserializer<'de>, T: TryFrom<Vec<u8>>>(
        deserializer: D,
    ) -> Result<T, D::Error> {
        let value = String::deserialize(deserializer)?;
        let bytes = STANDARD.decode(value).map_err(serde::de::Error::custom)?;
        T::try_from(bytes).map_err(|_| serde::de::Error::custom("invalid byte field length"))
    }

    pub fn serialize_optional<S: Serializer>(
        value: &Option<[u8; 64]>,
        serializer: S,
    ) -> Result<S::Ok, S::Error> {
        value
            .as_ref()
            .map(|bytes| STANDARD.encode(bytes))
            .serialize(serializer)
    }

    pub fn deserialize_optional<'de, D: Deserializer<'de>>(
        deserializer: D,
    ) -> Result<Option<[u8; 64]>, D::Error> {
        Option::<String>::deserialize(deserializer)?
            .map(|value| {
                let bytes = STANDARD.decode(value).map_err(serde::de::Error::custom)?;
                bytes
                    .try_into()
                    .map_err(|_| serde::de::Error::custom("anchor must contain 64 bytes"))
            })
            .transpose()
    }
}
