use std::path::PathBuf;

use axum::body::Body;
use axum::http::{header, HeaderMap, Method, StatusCode};
use axum::response::Response;
use tokio_util::io::ReaderStream;

use crate::error::{anyhow, Result};
use crate::local::files::{self, FilePresentation};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct ByteRange {
    start: u64,
    length: u64,
}

/// `Ok(None)` means no usable single range was requested. Unsupported or
/// malformed ranges are ignored per HTTP semantics; only a valid but
/// unsatisfiable range returns `Err(())` and becomes 416.
fn requested_byte_range(
    headers: &HeaderMap,
    size: u64,
) -> std::result::Result<Option<ByteRange>, ()> {
    let Some(value) = headers.get(header::RANGE) else {
        return Ok(None);
    };
    let Ok(value) = value.to_str() else {
        return Ok(None);
    };
    let Some(spec) = value.strip_prefix("bytes=") else {
        return Ok(None);
    };
    if spec.contains(',') {
        return Ok(None);
    }
    let Some((start, end)) = spec.split_once('-') else {
        return Ok(None);
    };
    if start.is_empty() {
        let Ok(suffix) = end.parse::<u64>() else {
            return Ok(None);
        };
        if suffix == 0 || size == 0 {
            return Err(());
        }
        let length = suffix.min(size);
        return Ok(Some(ByteRange {
            start: size - length,
            length,
        }));
    }
    let Ok(start) = start.parse::<u64>() else {
        return Ok(None);
    };
    if start >= size {
        return Err(());
    }
    let end = if end.is_empty() {
        size - 1
    } else {
        let Ok(end) = end.parse::<u64>() else {
            return Ok(None);
        };
        end.min(size - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some(ByteRange {
        start,
        length: end - start + 1,
    }))
}

fn range_not_satisfiable(size: u64) -> Result<Response> {
    Response::builder()
        .status(StatusCode::RANGE_NOT_SATISFIABLE)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_RANGE, format!("bytes */{size}"))
        .body(Body::empty())
        .map_err(|error| anyhow!("file response failed: {error}"))
}

fn response(
    type_path: &str,
    presentation: FilePresentation,
    size: u64,
    range: Option<ByteRange>,
    cache_control: &'static str,
    body: Body,
) -> Result<Response> {
    let selected_length = range.map_or(size, |range| range.length);
    let mut response = Response::builder()
        .status(if range.is_some() {
            StatusCode::PARTIAL_CONTENT
        } else {
            StatusCode::OK
        })
        .header(
            header::CONTENT_TYPE,
            files::content_type_for_path(type_path),
        )
        .header(
            header::CONTENT_DISPOSITION,
            files::content_disposition_for_path(type_path),
        )
        .header(header::CACHE_CONTROL, cache_control)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, selected_length.to_string())
        .header("x-content-type-options", "nosniff")
        .header("x-openresearch-presentation", presentation.as_str());
    if let Some(range) = range {
        response = response.header(
            header::CONTENT_RANGE,
            format!(
                "bytes {}-{}/{}",
                range.start,
                range.start + range.length - 1,
                size
            ),
        );
    }
    if matches!(
        files::content_type_for_path(type_path),
        "image/svg+xml" | "text/html; charset=utf-8" | "application/xml"
    ) {
        response = response.header(
            "content-security-policy",
            "sandbox; default-src 'none'; img-src data:; style-src 'unsafe-inline'",
        );
    }
    response
        .body(body)
        .map_err(|error| anyhow!("file response failed: {error}"))
}

fn range_for(
    method: &Method,
    headers: &HeaderMap,
    size: u64,
) -> std::result::Result<Option<ByteRange>, ()> {
    if method != Method::GET {
        return Ok(None);
    }
    requested_byte_range(headers, size)
}

/// `type_path` must be the path the bytes actually came from: content type and
/// disposition are derived from its extension, and an in-repo symlink would
/// otherwise let a requested name dictate the type of another file's contents.
pub async fn disk_response(
    type_path: &str,
    file: std::fs::File,
    presentation: FilePresentation,
    method: &Method,
    headers: &HeaderMap,
    cache_control: &'static str,
) -> Result<Response> {
    let mut file = tokio::fs::File::from_std(file);
    let metadata = file
        .metadata()
        .await
        .map_err(|error| anyhow!("stat failed: {error}"))?;
    let size = metadata.len();
    let range = match range_for(method, headers, size) {
        Ok(range) => range,
        Err(()) => return range_not_satisfiable(size),
    };
    if method == Method::HEAD || size == 0 {
        let mut result = response(
            type_path,
            presentation,
            size,
            range,
            cache_control,
            Body::empty(),
        )?;
        if let Ok(modified) = metadata.modified() {
            let nanos = modified
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos();
            result
                .headers_mut()
                .insert(header::ETAG, format!("W/\"{size}-{nanos}\"").parse()?);
        }
        return Ok(result);
    }
    use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _};
    let selection = range.unwrap_or(ByteRange {
        start: 0,
        length: size,
    });
    if selection.start > 0 {
        file.seek(std::io::SeekFrom::Start(selection.start))
            .await
            .map_err(|error| anyhow!("seek failed: {error}"))?;
    }
    response(
        type_path,
        presentation,
        size,
        range,
        cache_control,
        Body::from_stream(ReaderStream::new(file.take(selection.length))),
    )
}

/// See [`disk_response`] for `type_path`.
pub async fn git_response(
    type_path: &str,
    repo: PathBuf,
    spec: String,
    size: u64,
    presentation: FilePresentation,
    method: &Method,
    headers: &HeaderMap,
) -> Result<Response> {
    let range = match range_for(method, headers, size) {
        Ok(range) => range,
        Err(()) => return range_not_satisfiable(size),
    };
    if method == Method::HEAD || size == 0 {
        return response(
            type_path,
            presentation,
            size,
            range,
            "no-cache",
            Body::empty(),
        );
    }

    use std::process::Stdio;
    use tokio::io::AsyncReadExt as _;
    let mut child = tokio::process::Command::new("git")
        .current_dir(repo)
        .env("GIT_TERMINAL_PROMPT", "0")
        .args(["cat-file", "blob", &spec])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| anyhow!("Could not run git: {error}"))?;
    let mut stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("git stdout unavailable"))?;
    let selection = range.unwrap_or(ByteRange {
        start: 0,
        length: size,
    });
    if selection.start > 0 {
        if let Err(error) = tokio::io::copy(
            &mut (&mut stdout).take(selection.start),
            &mut tokio::io::sink(),
        )
        .await
        {
            let _ = child.kill().await;
            let _ = child.wait().await;
            return Err(anyhow!("git stream failed: {error}"));
        }
    }
    let reader = stdout.take(selection.length);
    tokio::spawn(async move {
        let _ = child.wait().await;
    });
    response(
        type_path,
        presentation,
        size,
        range,
        "no-cache",
        Body::from_stream(ReaderStream::new(reader)),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    fn range_headers(value: &str) -> HeaderMap {
        let mut headers = HeaderMap::new();
        headers.insert(header::RANGE, value.parse().unwrap());
        headers
    }

    async fn body(response: Response) -> Vec<u8> {
        to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap()
            .to_vec()
    }

    #[tokio::test]
    async fn head_validator_changes_for_same_size_rewrites() {
        let path = std::env::temp_dir().join(format!("orx-file-version-{}", uuid::Uuid::new_v4()));
        let mut versions = Vec::new();
        for (offset, content) in [(0, b"before"), (1, b"after!")] {
            std::fs::write(&path, content).unwrap();
            let file = std::fs::File::options().write(true).open(&path).unwrap();
            file.set_modified(std::time::UNIX_EPOCH + std::time::Duration::from_secs(100 + offset))
                .unwrap();
            for _ in 0..2 {
                let response = disk_response(
                    "file.txt",
                    std::fs::File::open(&path).unwrap(),
                    FilePresentation::Text,
                    &Method::HEAD,
                    &HeaderMap::new(),
                    "no-cache",
                )
                .await
                .unwrap();
                versions.push(response.headers()[header::ETAG].clone());
                assert!(body(response).await.is_empty());
            }
        }
        assert_eq!(versions[0], versions[1]);
        assert_ne!(versions[1], versions[2]);
        assert_eq!(versions[2], versions[3]);
        std::fs::remove_file(path).unwrap();
    }

    /// The typing contract `type_path` states: a symlink named `logo.png` must
    /// not pass a `.env` off as an image. The HTML preview's inlining allow-list
    /// reads this header to decide what it may hand an untrusted document.
    #[cfg(unix)]
    #[tokio::test]
    async fn types_a_symlink_by_the_path_it_resolved_to() {
        let dir = std::env::temp_dir().join(format!("orx-http-link-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("secret.env"), b"TOKEN=1").unwrap();
        std::os::unix::fs::symlink("secret.env", dir.join("logo.png")).unwrap();
        let resolved = std::fs::canonicalize(dir.join("logo.png")).unwrap();

        let response = disk_response(
            &resolved.to_string_lossy(),
            std::fs::File::open(&resolved).unwrap(),
            files::presentation_for_path(&resolved.to_string_lossy()),
            &Method::GET,
            &HeaderMap::new(),
            "no-cache",
        )
        .await
        .unwrap();
        assert_eq!(
            response.headers()[header::CONTENT_TYPE],
            "text/plain; charset=utf-8"
        );
        assert_eq!(response.headers()["x-openresearch-presentation"], "text");
        // The name asked for is what it would have been typed as.
        assert_eq!(files::content_type_for_path("logo.png"), "image/png");
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[tokio::test]
    async fn disk_responses_cover_full_head_range_and_empty_files() {
        let dir = std::env::temp_dir().join(format!("orx-http-file-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("sample.mp4");
        std::fs::write(&file, b"0123456789").unwrap();

        let full = disk_response(
            "sample.mp4",
            std::fs::File::open(&file).unwrap(),
            FilePresentation::Video,
            &Method::GET,
            &HeaderMap::new(),
            "no-cache",
        )
        .await
        .unwrap();
        assert_eq!(full.status(), StatusCode::OK);
        assert_eq!(full.headers()[header::CONTENT_LENGTH], "10");
        assert_eq!(full.headers()[header::CONTENT_TYPE], "video/mp4");
        assert_eq!(full.headers()[header::CONTENT_DISPOSITION], "inline");
        assert_eq!(full.headers()[header::ACCEPT_RANGES], "bytes");
        assert_eq!(full.headers()[header::CACHE_CONTROL], "no-cache");
        assert_eq!(full.headers()["x-content-type-options"], "nosniff");
        assert_eq!(full.headers()["x-openresearch-presentation"], "video");
        assert_eq!(body(full).await, b"0123456789");

        let partial = disk_response(
            "sample.mp4",
            std::fs::File::open(&file).unwrap(),
            FilePresentation::Video,
            &Method::GET,
            &range_headers("bytes=2-5"),
            "no-cache",
        )
        .await
        .unwrap();
        assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(partial.headers()[header::CONTENT_RANGE], "bytes 2-5/10");
        assert_eq!(body(partial).await, b"2345");

        let head = disk_response(
            "sample.mp4",
            std::fs::File::open(&file).unwrap(),
            FilePresentation::Video,
            &Method::HEAD,
            &range_headers("bytes=2-5"),
            "no-cache",
        )
        .await
        .unwrap();
        assert_eq!(head.status(), StatusCode::OK);
        assert_eq!(head.headers()[header::CONTENT_LENGTH], "10");
        assert!(body(head).await.is_empty());

        let multipart = disk_response(
            "sample.mp4",
            std::fs::File::open(&file).unwrap(),
            FilePresentation::Video,
            &Method::GET,
            &range_headers("bytes=0-1,4-5"),
            "no-cache",
        )
        .await
        .unwrap();
        assert_eq!(multipart.status(), StatusCode::OK);
        assert_eq!(body(multipart).await, b"0123456789");

        let unsatisfiable = disk_response(
            "sample.mp4",
            std::fs::File::open(&file).unwrap(),
            FilePresentation::Video,
            &Method::GET,
            &range_headers("bytes=10-"),
            "no-cache",
        )
        .await
        .unwrap();
        assert_eq!(unsatisfiable.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(unsatisfiable.headers()[header::CONTENT_RANGE], "bytes */10");

        let empty = dir.join("empty.txt");
        std::fs::write(&empty, []).unwrap();
        let empty = disk_response(
            "empty.txt",
            std::fs::File::open(empty).unwrap(),
            FilePresentation::Text,
            &Method::GET,
            &HeaderMap::new(),
            "no-cache",
        )
        .await
        .unwrap();
        assert_eq!(empty.status(), StatusCode::OK);
        assert_eq!(empty.headers()[header::CONTENT_LENGTH], "0");
        assert!(body(empty).await.is_empty());

        let svg = dir.join("active.svg");
        std::fs::write(&svg, b"<svg></svg>").unwrap();
        let svg = disk_response(
            "active.svg",
            std::fs::File::open(svg).unwrap(),
            FilePresentation::Image,
            &Method::HEAD,
            &HeaderMap::new(),
            "no-cache",
        )
        .await
        .unwrap();
        assert_eq!(svg.headers()[header::CONTENT_TYPE], "image/svg+xml");
        assert!(svg.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .starts_with("sandbox;"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn git_response_returns_exact_requested_bytes() {
        let dir = std::env::temp_dir().join(format!("orx-http-git-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        for args in [
            &["init", "-q", "-b", "main"][..],
            &["config", "user.name", "orx-test"][..],
            &["config", "user.email", "orx-test@example.com"][..],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        std::fs::write(dir.join("sample.bin"), b"abcdefghij").unwrap();
        for args in [
            &["add", "sample.bin"][..],
            &["-c", "commit.gpgsign=false", "commit", "-q", "-m", "sample"][..],
        ] {
            assert!(std::process::Command::new("git")
                .current_dir(&dir)
                .args(args)
                .status()
                .unwrap()
                .success());
        }
        let response = git_response(
            "sample.bin",
            dir.clone(),
            "HEAD:sample.bin".to_string(),
            10,
            FilePresentation::Download,
            &Method::GET,
            &range_headers("bytes=-3"),
        )
        .await
        .unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()[header::CONTENT_RANGE], "bytes 7-9/10");
        assert_eq!(body(response).await, b"hij");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
