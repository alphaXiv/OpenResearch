use orx_host_proto::*;
use std::io::{self, Cursor, Read};

#[test]
fn golden_request_and_response() {
    let request = Message {
        id: 7,
        body: Request::Hello {
            proto: PROTOCOL_VERSION,
        },
    };
    let json = format!(r#"{{"id":7,"body":{{"op":"hello","proto":{PROTOCOL_VERSION}}}}}"#);
    let mut golden = (json.len() as u32).to_be_bytes().to_vec();
    golden.extend_from_slice(json.as_bytes());
    let mut encoded = Vec::new();
    write_frame(&mut encoded, &request).unwrap();
    assert_eq!(encoded, golden);
    assert_eq!(
        read_frame::<Message<Request>>(&mut golden.as_slice()).unwrap(),
        Some(request)
    );

    let response = Message {
        id: 7,
        body: Response::Cancel {
            outcome: CancelAck::Accepted,
        },
    };
    let json = br#"{"id":7,"body":{"op":"cancel","outcome":"accepted"}}"#;
    let mut golden = (json.len() as u32).to_be_bytes().to_vec();
    golden.extend_from_slice(json);
    encoded.clear();
    write_frame(&mut encoded, &response).unwrap();
    assert_eq!(encoded, golden);
    assert_eq!(
        read_frame::<Message<Response>>(&mut golden.as_slice()).unwrap(),
        Some(response)
    );
}

#[test]
fn launch_reuses_the_intent_without_changing_its_wire_shape() {
    let json =
        r#"{"op":"launch","run":"r1","digest":"sha","run_sh":"true","env":{},"source":null}"#;
    let request: Request = serde_json::from_str(json).unwrap();
    assert!(matches!(&request, Request::Launch(intent) if intent.run == "r1"));
    assert_eq!(serde_json::to_string(&request).unwrap(), json);
}

#[test]
fn golden_receipt_and_summary() {
    let json = r#"{"state":{"phase":"exited","receipt":{"code":0,"signal":null,"reason":"exited","processes":[],"outcome":{"kind":"verified"}}},"log_len":23,"notify_len":0,"cleanup":null}"#;
    let summary: Summary = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&summary).unwrap(), json);
    assert!(matches!(
        summary.state,
        RunState::Exited {
            receipt: ExitReceipt { code: Some(0), .. }
        }
    ));
}

#[test]
fn eof_truncation_and_invalid_lengths() {
    assert!(read_frame::<Request>(&mut [].as_slice()).unwrap().is_none());
    for bytes in [vec![0], vec![0, 0, 0], vec![0, 0, 0, 2, b'{']] {
        assert_eq!(
            read_frame::<Request>(&mut bytes.as_slice())
                .unwrap_err()
                .kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
    for len in [0, MAX_FRAME_LEN as u32 + 1, u32::MAX] {
        assert_eq!(
            read_frame::<Request>(&mut len.to_be_bytes().as_slice())
                .unwrap_err()
                .kind(),
            io::ErrorKind::InvalidData
        );
    }
    assert!(read_frame::<Request>(&mut [0, 0, 0, 1, b'?'].as_slice()).is_err());
}

struct Fragmented<'a>(&'a [u8]);
impl Read for Fragmented<'_> {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let len = bytes.len().min(1);
        self.0.read(&mut bytes[..len])
    }
}

#[test]
fn fragmented_and_consecutive_frames() {
    let mut bytes = Vec::new();
    for id in 0..12 {
        write_frame(
            &mut bytes,
            &Message {
                id,
                body: Request::Hello {
                    proto: orx_host_proto::PROTOCOL_VERSION,
                },
            },
        )
        .unwrap();
    }
    let mut reader = Fragmented(&bytes);
    for id in 0..12 {
        assert_eq!(
            read_frame::<Message<Request>>(&mut reader)
                .unwrap()
                .unwrap()
                .id,
            id
        );
    }
    assert!(read_frame::<Request>(&mut reader).unwrap().is_none());
}

#[test]
fn random_frames_and_roundtrips() {
    let mut seed = 0x7ad3_2915_1726_4401u64;
    let mut next = || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed
    };
    for _ in 0..10_000 {
        let size = (next() % 1024) as usize;
        let bytes: Vec<_> = (0..size).map(|_| next() as u8).collect();
        let mut frame = (size as u32).to_be_bytes().to_vec();
        frame.extend_from_slice(&bytes);
        let _ = read_frame::<Message<Request>>(&mut Cursor::new(frame));
        let offset = next();
        let request = Message {
            id: next(),
            body: Request::ReadLog {
                target: LogTarget::Run(format!("run-{}", next())),
                file: LogFile::Log,
                ident: None,
                range: ByteRange {
                    start: offset,
                    end: offset.saturating_add(next() % MAX_PAGE_LEN),
                },
                anchor: Some([next() as u8; 64]),
            },
        };
        let mut encoded = Vec::new();
        write_frame(&mut encoded, &request).unwrap();
        assert_eq!(
            read_frame::<Message<Request>>(&mut encoded.as_slice()).unwrap(),
            Some(request)
        );
    }
}

#[test]
fn malformed_anchor_is_rejected() {
    let mut json = serde_json::to_value(Request::ReadLog {
        target: LogTarget::Run("r".into()),
        file: LogFile::Log,
        ident: None,
        range: ByteRange { start: 0, end: 1 },
        anchor: Some([0; 64]),
    })
    .unwrap();
    json["anchor"] = serde_json::json!([0, 1]);
    assert!(serde_json::from_value::<Request>(json).is_err());
}

#[test]
fn legacy_container_descriptor_keeps_its_shipped_shape() {
    let json = r#"{"reference":"worker","id":"abc123","startedAt":"generation","runDir":"/home/user/.orx/runs/r1"}"#;
    let container: ContainerRun = serde_json::from_str(json).unwrap();
    assert_eq!(serde_json::to_string(&container).unwrap(), json);
    let request = Request::CancelLegacy {
        dir: "~/.orx/runs/r1".into(),
        container: Some(container),
    };
    let mut bytes = vec![];
    write_frame(&mut bytes, &request).unwrap();
    assert_eq!(
        read_frame::<Request>(&mut bytes.as_slice()).unwrap(),
        Some(request)
    );
}

#[test]
fn cleanup_wire_and_verified_receipt_are_canonical() {
    let request: Request = serde_json::from_str(r#"{"op":"cleanup","run":"r1"}"#).unwrap();
    assert_eq!(request, Request::Cleanup { run: "r1".into() });
    assert_eq!(
        serde_json::to_string(&Response::Cleanup {
            outcome: CleanupAck::NothingLingering
        })
        .unwrap(),
        r#"{"op":"cleanup","outcome":"nothing_lingering"}"#
    );
    assert_eq!(
        serde_json::to_string(&CancelOutcome::Verified).unwrap(),
        r#"{"kind":"verified"}"#
    );
}

#[test]
fn maximum_byte_payloads_fit_frames_with_base64_expansion() {
    let bytes = vec![255; MAX_PAGE_LEN as usize];
    let requests = [
        Request::Exec {
            command: "cat".into(),
            stdin: bytes.clone(),
            timeout_ms: 1,
        },
        Request::PutBlob {
            sha: "a".repeat(64),
            offset: u64::MAX,
            bytes: bytes[..256 * 1024].to_vec(),
        },
    ];
    for body in requests {
        let message = Message { id: u64::MAX, body };
        let mut frame = Vec::new();
        write_frame(&mut frame, &message).unwrap();
        assert!(frame.len() < MAX_FRAME_LEN);
        assert_eq!(
            read_frame::<Message<Request>>(&mut frame.as_slice()).unwrap(),
            Some(message)
        );
    }
    let responses = [
        Response::Exec {
            result: ExecResult {
                code: Some(0),
                signal: None,
                timed_out: false,
                stdout: bytes.clone(),
                stderr: bytes.clone(),
                stdout_truncated: false,
                stderr_truncated: false,
            },
        },
        Response::ReadLog {
            page: LogPage::Data {
                len: None,
                ident: FileIdent {
                    dev: u64::MAX,
                    ino: u64::MAX,
                },
                start: u64::MAX,
                bytes,
                eof: false,
            },
        },
    ];
    for body in responses {
        let message = Message { id: u64::MAX, body };
        let mut frame = Vec::new();
        write_frame(&mut frame, &message).unwrap();
        assert!(frame.len() < MAX_FRAME_LEN);
        assert_eq!(
            read_frame::<Message<Response>>(&mut frame.as_slice()).unwrap(),
            Some(message)
        );
    }
    for invalid in [
        serde_json::json!([0, 255]),
        serde_json::json!("!!!!"),
        serde_json::json!("/x=="),
    ] {
        let value = serde_json::json!({"op":"put_blob", "sha":"a", "offset":0, "bytes":invalid});
        assert!(serde_json::from_value::<Request>(value).is_err());
    }
}
