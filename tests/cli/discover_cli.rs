use std::io::{Read, Write};

const FEEDBACK: &str = "\"errorlist\":{\"phrasesnotfound\":[\"zzqqxxyy\"]}";
const ARTICLES: &str = "<PubmedArticleSet><PubmedArticle><MedlineCitation><PMID>111</PMID><Article><ArticleTitle>T</ArticleTitle></Article></MedlineCitation></PubmedArticle></PubmedArticleSet>";

/// Serves `esearch` then, when it found ids, `efetch`; returns the base URL.
fn pubmed_server(idlist: &str) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let search = format!("{{\"esearchresult\":{{\"idlist\":{idlist},{FEEDBACK}}}}}");
    let requests = if idlist == "[]" { 1 } else { 2 };
    std::thread::spawn(move || {
        for body in [search.as_str(), ARTICLES].into_iter().take(requests) {
            let (mut stream, _) = listener.accept().unwrap();
            let _request = stream.read(&mut [0u8; 4096]).unwrap();
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
    });
    url
}

#[test]
fn pubmed_feedback_goes_to_stderr_and_stdout_stays_json() {
    for (idlist, papers) in [("[\"111\"]", 1), ("[]", 0)] {
        let root = std::env::temp_dir().join(format!("orx-discover-cli-{}", uuid::Uuid::new_v4()));
        let output = crate::home::isolate_home(
            &mut std::process::Command::new(env!("CARGO_BIN_EXE_orx")),
            &root,
        )
        .env("XDG_CONFIG_HOME", root.join("config"))
        .env("ORX_DATA_DIR", root.join("data"))
        .env("ORX_NO_UPDATE_CHECK", "1")
        .env("PUBMED_API_URL", pubmed_server(idlist))
        .env("NO_PROXY", "127.0.0.1")
        .env_remove("HTTP_PROXY")
        .env_remove("HTTPS_PROXY")
        .env_remove("ALL_PROXY")
        .args(["--no-telemetry", "discover", "pubmed", "crispr"])
        .output()
        .unwrap();
        let _ = std::fs::remove_dir_all(root);

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert!(stderr.contains("zzqqxxyy"), "{stderr}");
        let hits: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(hits.len(), papers);
    }
}
