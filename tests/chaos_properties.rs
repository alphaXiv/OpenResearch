#![cfg(unix)]

use orx_host_proto::*;
use std::io::{self, Read};
#[path = "../src/engine/runtime/chaos/rng.rs"]
mod rng;
use rng::Rng;

struct Fragments<'a> {
    bytes: &'a [u8],
    width: usize,
    interrupted: bool,
}
impl Read for Fragments<'_> {
    fn read(&mut self, out: &mut [u8]) -> io::Result<usize> {
        self.interrupted = !self.interrupted;
        if self.interrupted {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let len = out.len().min(self.width);
        self.bytes.read(&mut out[..len])
    }
}

fn check_frame<T: serde::de::DeserializeOwned + serde::Serialize + std::fmt::Debug + PartialEq>(
    frame: &[u8],
    width: usize,
    seed: u64,
    case: usize,
) {
    let expected = if frame.is_empty() {
        Ok(None)
    } else if frame.len() < 4 {
        Err(io::ErrorKind::UnexpectedEof)
    } else {
        let len = u32::from_be_bytes(frame[..4].try_into().unwrap()) as usize;
        if len == 0 || len > MAX_FRAME_LEN {
            Err(io::ErrorKind::InvalidData)
        } else if frame.len() - 4 < len {
            Err(io::ErrorKind::UnexpectedEof)
        } else {
            serde_json::from_slice::<T>(&frame[4..4 + len])
                .map(Some)
                .map_err(|_| io::ErrorKind::InvalidData)
        }
    };
    let mut fragments = Fragments {
        bytes: frame,
        width,
        interrupted: false,
    };
    let actual = read_frame::<T>(&mut fragments).map_err(|e| e.kind());
    assert_eq!(actual, expected, "seed={seed} case={case} bytes={frame:?}");
    if let Ok(Some(parsed)) = actual {
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &parsed).unwrap();
        assert_eq!(
            read_frame::<T>(&mut encoded.as_slice()).unwrap(),
            Some(parsed)
        );
    }
}

fn request(rng: &mut Rng) -> Message<Request> {
    let run = format!("r-{}", rng.next());
    let body = match rng.next() % 8 {
        0 => Request::Hello {
            proto: rng.next() as u32,
        },
        1 => Request::Launch(LaunchIntent {
            run,
            digest: format!("{:x}", rng.next()),
            run_sh: "printf 'λ\\n'".into(),
            env: std::collections::BTreeMap::from([("KEY".into(), format!("{}", rng.next()))]),
            source: None,
            cancel_sh: None,
        }),
        2 => Request::ReadLog {
            target: LogTarget::Run(run),
            file: LogFile::Log,
            ident: None,
            range: ByteRange {
                start: rng.next(),
                end: rng.next(),
            },
            anchor: Some([rng.next() as u8; 64]),
        },
        3 => Request::Exec {
            command: run,
            stdin: (0..rng.next() % 128).map(|_| rng.next() as u8).collect(),
            timeout_ms: rng.next() as u32,
        },
        4 => Request::PutBlob {
            sha: run,
            offset: rng.next(),
            bytes: vec![rng.next() as u8; rng.next() as usize % 128],
        },
        5 => Request::Cancel { run },
        6 => Request::Changes {
            watch: vec![],
            follow_logs: vec![run],
            wait_ms: rng.next() as u32,
        },
        _ => Request::SlurmSubmit {
            intent: LaunchIntent {
                run,
                digest: format!("{:x}", rng.next()),
                run_sh: "true".into(),
                env: Default::default(),
                source: None,
                cancel_sh: None,
            },
            directives: "#SBATCH --job-name=r\n".into(),
        },
    };
    Message {
        id: rng.next(),
        body,
    }
}

fn response(rng: &mut Rng) -> Message<Response> {
    let body = match rng.next() % 8 {
        0 => Response::Hello {
            proto: rng.next() as u32,
            binary_sha: format!("{}", rng.next()),
            boot_id: "boot".into(),
            os: "Linux".into(),
            arch: "aarch64".into(),
            kernel: "test".into(),
        },
        1 => Response::ReadLog {
            page: LogPage::Data {
                len: None,
                ident: FileIdent {
                    dev: rng.next(),
                    ino: rng.next(),
                },
                start: rng.next(),
                bytes: (0..rng.next() % 256).map(|_| rng.next() as u8).collect(),
                eof: rng.next() & 1 == 0,
            },
        },
        2 => Response::ReadLog {
            page: LogPage::Reset { info: None },
        },
        3 => Response::Error {
            code: ErrorCode::Io,
            message: format!("error-{}\nλ", rng.next()),
        },
        4 => Response::Cancel {
            outcome: CancelAck::Accepted,
        },
        5 => Response::Launch {
            summary: Summary {
                started_at: None,
                state: RunState::Unreconciled {
                    detail: "missing receipt".into(),
                },
                log_len: Some(rng.next()),
                notify_len: rng.next(),
                cleanup: None,
            },
        },
        6 => Response::Exec {
            result: ExecResult {
                code: Some(rng.next() as i32),
                signal: None,
                timed_out: false,
                stdout: vec![rng.next() as u8; 32],
                stderr: vec![],
                stdout_truncated: false,
                stderr_truncated: false,
            },
        },
        _ => Response::SlurmQueue { jobs: vec![] },
    };
    Message {
        id: rng.next(),
        body,
    }
}

#[test]
fn seeded_consecutive_frames_preserve_boundaries() {
    let seed = 0x5513827cc8bed129;
    let mut rng = Rng(seed);
    for case in 0..1000 {
        let requests = [request(&mut rng), request(&mut rng)];
        let responses = [response(&mut rng), response(&mut rng)];
        let mut bytes = Vec::new();
        for value in &requests {
            write_frame(&mut bytes, value).unwrap();
        }
        for value in &responses {
            write_frame(&mut bytes, value).unwrap();
        }
        let mut reader = Fragments {
            bytes: &bytes,
            width: 1 + rng.next() as usize % 31,
            interrupted: false,
        };
        for value in requests {
            assert_eq!(
                read_frame::<Message<Request>>(&mut reader).unwrap(),
                Some(value),
                "seed={seed} case={case}"
            );
        }
        for value in responses {
            assert_eq!(
                read_frame::<Message<Response>>(&mut reader).unwrap(),
                Some(value),
                "seed={seed} case={case}"
            );
        }
        assert!(read_frame::<Message<Request>>(&mut reader)
            .unwrap()
            .is_none());
    }
}

#[test]
fn seeded_frame_decoder_matches_length_and_json_oracle() {
    let seed = 0xb857ac092a772813;
    let mut rng = Rng(seed);
    for case in 0..5000 {
        let mut frame = Vec::new();
        if rng.next() & 1 == 0 {
            write_frame(&mut frame, &request(&mut rng)).unwrap();
        } else {
            write_frame(&mut frame, &response(&mut rng)).unwrap();
        }
        match rng.next() % 5 {
            0 => {
                let len = rng.next() as usize % frame.len();
                frame.truncate(len);
            }
            1 => {
                let index = rng.next() as usize % frame.len();
                frame[index] ^= (rng.next() as u8) | 1;
            }
            2 => {
                let length = [0, MAX_FRAME_LEN as u32 + 1, u32::MAX][rng.next() as usize % 3];
                frame[..4].copy_from_slice(&length.to_be_bytes());
            }
            3 => {
                for byte in &mut frame[4..] {
                    *byte = rng.next() as u8;
                }
            }
            _ => {}
        }
        let width = 1 + rng.next() as usize % 17;
        check_frame::<Message<Request>>(&frame, width, seed, case);
        check_frame::<Message<Response>>(&frame, width, seed, case);
    }
}

#[test]
fn seeded_exit_and_wrapper_receipts_survive_roundtrips_and_reject_corruption() {
    let seed = 0x29ae8175c385bd08;
    let mut rng = Rng(seed);
    let directory =
        std::env::temp_dir().join(format!("orx-receipt-properties-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir(&directory).unwrap();
    struct Cleanup(std::path::PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    let _cleanup = Cleanup(directory.clone());
    for case in 0..1000 {
        let identity = ProcessIdentity {
            node: format!("node-{}", rng.next()),
            boot_id: format!("boot-{}", rng.next()),
            pid: rng.next() as u32,
            starttime: rng.next(),
        };
        let receipt = ExitReceipt {
            finished_at: None,
            code: if rng.next() & 1 == 0 {
                Some(rng.next() as i32)
            } else {
                None
            },
            signal: if rng.next() & 1 == 0 {
                Some(rng.next() as i32)
            } else {
                None
            },
            reason: match rng.next() % 3 {
                0 => ExitReason::Exited,
                1 => ExitReason::Cancelled,
                _ => ExitReason::SpawnFailed,
            },
            processes: (0..rng.next() % 4)
                .map(|_| Survivor {
                    pid: rng.next() as u32,
                    starttime: rng.next(),
                    state: "S".into(),
                })
                .collect(),
            outcome: match rng.next() % 3 {
                0 => CancelOutcome::Verified,
                1 => CancelOutcome::Survivors,
                _ => CancelOutcome::Unverifiable {
                    unreadable: vec![rng.next() as u32],
                },
            },
            detail: None,
        };
        std::fs::write(
            directory.join("wrapper.json"),
            serde_json::to_vec(&identity).unwrap(),
        )
        .unwrap();
        std::fs::write(
            directory.join("exit.json"),
            serde_json::to_vec(&receipt).unwrap(),
        )
        .unwrap();
        assert_eq!(
            orx_host::read_json::<ProcessIdentity>(&directory.join("wrapper.json")).unwrap(),
            identity,
            "seed={seed} case={case}"
        );
        assert_eq!(
            orx_host::wrapper::receipt(&directory).unwrap(),
            Some(receipt.clone()),
            "seed={seed} case={case}"
        );
        for (name, value) in [
            ("wrapper.json", serde_json::to_value(&identity).unwrap()),
            ("exit.json", serde_json::to_value(&receipt).unwrap()),
        ] {
            // Every required field is independently deleted or given an impossible type.
            for key in value.as_object().unwrap().keys() {
                for replacement in [None, Some(serde_json::json!({"invalid":true}))] {
                    let mut corrupted = value.clone();
                    if let Some(replacement) = replacement {
                        corrupted[key] = replacement;
                    } else {
                        corrupted.as_object_mut().unwrap().remove(key);
                    }
                    let bytes = serde_json::to_vec(&corrupted).unwrap();
                    std::fs::write(directory.join(name), &bytes).unwrap();
                    if name == "wrapper.json" {
                        let expected = serde_json::from_slice::<ProcessIdentity>(&bytes);
                        assert_eq!(
                            orx_host::read_json::<ProcessIdentity>(&directory.join(name)).is_ok(),
                            expected.is_ok(),
                            "seed={seed} case={case} key={key}"
                        );
                    } else {
                        let expected = serde_json::from_slice::<ExitReceipt>(&bytes);
                        assert_eq!(
                            orx_host::wrapper::receipt(&directory).is_ok(),
                            expected.is_ok(),
                            "seed={seed} case={case} key={key}"
                        );
                    }
                }
            }
            let bytes = serde_json::to_vec(&value).unwrap();
            let end = rng.next() as usize % bytes.len();
            std::fs::write(directory.join(name), &bytes[..end]).unwrap();
            if name == "wrapper.json" {
                assert!(orx_host::read_json::<ProcessIdentity>(&directory.join(name)).is_err());
            } else {
                assert!(orx_host::wrapper::receipt(&directory).is_err());
            }
        }
    }
}

#[test]
fn seeded_byte_fields_use_base64_and_round_trip_arbitrary_bytes() {
    let mut rng = Rng(0x7a3f819ceab20d64);
    for _ in 0..512 {
        let bytes: Vec<_> = (0..rng.next() % 2048).map(|_| rng.next() as u8).collect();
        for body in [
            Request::Exec {
                command: "cat".into(),
                stdin: bytes.clone(),
                timeout_ms: 1,
            },
            Request::PutBlob {
                sha: "sha".into(),
                offset: 0,
                bytes: bytes.clone(),
            },
            Request::ReadLog {
                target: LogTarget::Run("r".into()),
                file: LogFile::Log,
                ident: None,
                range: ByteRange { start: 0, end: 64 },
                anchor: Some(std::array::from_fn(|_| rng.next() as u8)),
            },
        ] {
            let value = serde_json::to_value(&body).unwrap();
            for key in ["stdin", "bytes", "anchor"] {
                if let Some(value) = value.get(key) {
                    assert!(value.is_string());
                }
            }
            assert_eq!(serde_json::from_value::<Request>(value).unwrap(), body);
        }
        let result = ExecResult {
            code: Some(0),
            signal: None,
            timed_out: false,
            stdout: bytes.clone(),
            stderr: bytes,
            stdout_truncated: false,
            stderr_truncated: false,
        };
        let value = serde_json::to_value(&result).unwrap();
        assert!(value["stdout"].is_string());
        assert!(value["stderr"].is_string());
        assert_eq!(serde_json::from_value::<ExecResult>(value).unwrap(), result);
    }
}
