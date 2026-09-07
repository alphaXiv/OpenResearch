//! Read the Overleaf editor session cookie from a browser already signed in,
//! so the live sync (`crate::local::overleaf_live`) need not have it pasted.
//!
//! The cookie is HttpOnly, so no page can hand it over: the value is read from
//! the browser's own on-disk cookie store, on this machine, at the user's
//! request. Chromium encrypts each cookie with AES-128-CBC under a key kept in
//! the login Keychain; reading it asks macOS for that key, which is the one
//! prompt the user sees. Firefox stores cookies in the clear. Both are what
//! tools like `browser_cookie3` read.
//!
//! macOS only: the Keychain and these paths are Apple's. Elsewhere the import
//! is unavailable and the paste remains.

#[cfg(target_os = "macos")]
mod imp {
    use std::path::{Path, PathBuf};
    use std::process::Command;

    use crate::error::{anyhow, Result};

    /// The cookie names Overleaf's editor authenticates with, cloud first.
    const SESSION_COOKIES: &[&str] = &["overleaf_session2", "overleaf.sid", "sharelatex.sid"];

    /// A browser whose store was read, and the `name=value` found in it.
    pub struct Imported {
        pub source: String,
        pub cookie: String,
    }

    /// The Overleaf session cookie from whichever signed-in browser has one for
    /// `host`, or `None` when no browser holds one. An unreadable store (a
    /// browser that is not installed, a locked file) is skipped, not an error;
    /// the error case is reserved for when a store was found but its key could
    /// not be obtained, so the caller can say why.
    pub fn import_session(host: &str) -> Result<Option<Imported>> {
        let mut denied = None;
        for browser in chromium_browsers() {
            match browser.read(host) {
                Ok(Some(cookie)) => {
                    return Ok(Some(Imported {
                        source: browser.name.to_string(),
                        cookie,
                    }))
                }
                Ok(None) => {}
                Err(e) => denied = denied.or(Some(e)),
            }
        }
        match firefox_session(host) {
            Ok(Some(cookie)) => {
                return Ok(Some(Imported {
                    source: "Firefox".to_string(),
                    cookie,
                }))
            }
            Ok(None) => {}
            Err(e) => denied = denied.or(Some(e)),
        }
        match denied {
            Some(e) => Err(e),
            None => Ok(None),
        }
    }

    fn home() -> PathBuf {
        dirs::home_dir().unwrap_or_default()
    }

    fn unix_now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    /// Chromium `expires_utc` is microseconds since 1601; `0` is a session
    /// cookie, which never expires on its own.
    fn chromium_expired(expires_utc: i64) -> bool {
        const EPOCH_1601_TO_1970_SECS: i64 = 11_644_473_600;
        expires_utc != 0 && expires_utc < (unix_now() + EPOCH_1601_TO_1970_SECS) * 1_000_000
    }

    /// Firefox `expiry` is seconds since 1970; `0` is a session cookie.
    fn firefox_expired(expiry: i64) -> bool {
        expiry != 0 && expiry < unix_now()
    }

    /// `host` matches a stored cookie's host when they are equal or the stored
    /// host is a leading-dot parent (`.overleaf.com` covers `www.overleaf.com`).
    fn host_matches(cookie_host: &str, host: &str) -> bool {
        let cookie_host = cookie_host.trim_start_matches('.');
        host == cookie_host || host.ends_with(&format!(".{cookie_host}"))
    }

    /// The session cookie to keep out of a store's rows: a known session name,
    /// non-empty, and the latest-expiring when a name repeats across profiles.
    fn pick(
        rows: Vec<(String, String, i64)>,
        host: &str,
        rows_host: impl Fn(usize) -> String,
    ) -> Option<String> {
        // Preferred cookie name first (lowest rank), then the latest expiry.
        let mut best: Option<(usize, (usize, std::cmp::Reverse<i64>))> = None;
        for (i, (name, value, expiry)) in rows.iter().enumerate() {
            let Some(rank) = SESSION_COOKIES.iter().position(|c| c == name) else {
                continue;
            };
            if value.is_empty() || !host_matches(&rows_host(i), host) {
                continue;
            }
            let key = (rank, std::cmp::Reverse(*expiry));
            if best.is_none_or(|(_, best_key)| key < best_key) {
                best = Some((i, key));
            }
        }
        best.map(|(i, _)| format!("{}={}", rows[i].0, rows[i].1))
    }

    // --- reading a sqlite cookie store -------------------------------------

    /// Open a copy: the browser keeps its own file locked, and a copy also
    /// keeps this read from touching the store the browser is writing.
    fn read_sqlite<T>(
        path: &Path,
        query: &str,
        row: impl Fn(&rusqlite::Row) -> rusqlite::Result<T>,
    ) -> Result<Vec<T>> {
        if !path.exists() {
            return Ok(Vec::new());
        }
        let copy =
            std::env::temp_dir().join(format!("orx-cookies-{}.sqlite", uuid::Uuid::new_v4()));
        std::fs::copy(path, &copy)?;
        for suffix in ["-wal", "-shm"] {
            let sidecar = path.with_file_name(format!(
                "{}{suffix}",
                path.file_name().unwrap_or_default().to_string_lossy()
            ));
            if sidecar.exists() {
                let _ = std::fs::copy(&sidecar, format!("{}{suffix}", copy.display()));
            }
        }
        let result = (|| {
            let conn = rusqlite::Connection::open_with_flags(
                &copy,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )?;
            let mut stmt = conn.prepare(query)?;
            let rows = stmt
                .query_map([], |r| row(r))?
                .collect::<rusqlite::Result<Vec<T>>>()?;
            Ok(rows)
        })();
        let _ = std::fs::remove_file(&copy);
        result.map_err(|e: rusqlite::Error| anyhow!("could not read the cookie store: {e}"))
    }

    // --- Chromium family ---------------------------------------------------

    struct Chromium {
        name: &'static str,
        /// Under `~/Library/Application Support`.
        support: &'static str,
        /// The Keychain item: service, then account.
        keychain: (&'static str, &'static str),
    }

    fn chromium_browsers() -> Vec<Chromium> {
        vec![
            Chromium {
                name: "Google Chrome",
                support: "Google/Chrome",
                keychain: ("Chrome Safe Storage", "Chrome"),
            },
            Chromium {
                name: "Microsoft Edge",
                support: "Microsoft Edge",
                keychain: ("Microsoft Edge Safe Storage", "Microsoft Edge"),
            },
            Chromium {
                name: "Brave",
                support: "BraveSoftware/Brave-Browser",
                keychain: ("Brave Safe Storage", "Brave"),
            },
            Chromium {
                name: "Arc",
                support: "Arc/User Data",
                keychain: ("Arc Safe Storage", "Arc"),
            },
            Chromium {
                name: "Vivaldi",
                support: "Vivaldi",
                keychain: ("Vivaldi Safe Storage", "Vivaldi"),
            },
            Chromium {
                name: "Chromium",
                support: "Chromium",
                keychain: ("Chromium Safe Storage", "Chromium"),
            },
        ]
    }

    impl Chromium {
        fn root(&self) -> PathBuf {
            home()
                .join("Library/Application Support")
                .join(self.support)
        }

        /// Every profile's cookie file. Chromium moved cookies under `Network/`,
        /// so both are tried, across `Default` and each `Profile N`.
        fn cookie_files(&self) -> Vec<PathBuf> {
            let root = self.root();
            let Ok(entries) = std::fs::read_dir(&root) else {
                return Vec::new();
            };
            let mut files = Vec::new();
            for entry in entries.flatten() {
                if !entry.path().is_dir() {
                    continue;
                }
                for tail in ["Network/Cookies", "Cookies"] {
                    let file = entry.path().join(tail);
                    if file.is_file() {
                        files.push(file);
                    }
                }
            }
            files
        }

        fn read(&self, host: &str) -> Result<Option<String>> {
            // The name and host are stored in the clear; only the value is
            // encrypted. Scanning them first means the Keychain is asked for
            // the key — the one prompt the user sees — only when this browser
            // actually holds a live Overleaf cookie, not for every install.
            let mut candidates: Vec<(String, String, Vec<u8>, i64)> = Vec::new();
            for file in self.cookie_files() {
                let read = read_sqlite(
                    &file,
                    "SELECT host_key, name, encrypted_value, expires_utc FROM cookies",
                    |r| {
                        Ok((
                            r.get::<_, String>(0)?,
                            r.get::<_, String>(1)?,
                            r.get::<_, Vec<u8>>(2)?,
                            r.get::<_, i64>(3)?,
                        ))
                    },
                )?;
                for (host_key, name, encrypted, expiry) in read {
                    if host_matches(&host_key, host)
                        && SESSION_COOKIES.contains(&name.as_str())
                        && !chromium_expired(expiry)
                    {
                        candidates.push((host_key, name, encrypted, expiry));
                    }
                }
            }
            if candidates.is_empty() {
                return Ok(None);
            }
            let key = self.key()?;
            let mut rows: Vec<(String, String, i64)> = Vec::new();
            let mut hosts: Vec<String> = Vec::new();
            for (host_key, name, encrypted, expiry) in candidates {
                if let Some(value) = decrypt(&encrypted, &key) {
                    hosts.push(host_key);
                    rows.push((name, value, expiry));
                }
            }
            let hosts_clone = hosts.clone();
            Ok(pick(rows, host, move |i| hosts_clone[i].clone()))
        }

        /// The AES key the store is encrypted with. `security` prints the
        /// Keychain value and, on first use, shows the system access prompt.
        fn key(&self) -> Result<[u8; 16]> {
            let (service, account) = self.keychain;
            let output = Command::new("security")
                .args(["find-generic-password", "-w", "-s", service, "-a", account])
                .output()
                .map_err(|e| anyhow!("could not ask the Keychain for {}'s key: {e}", self.name))?;
            if !output.status.success() {
                return Err(anyhow!(
                    "{} would not release its cookie key from the Keychain.",
                    self.name
                ));
            }
            let secret = String::from_utf8_lossy(&output.stdout);
            Ok(derive_key(secret.trim().as_bytes()))
        }
    }

    /// PBKDF2-HMAC-SHA1 with Chromium's fixed macOS salt and round count.
    fn derive_key(secret: &[u8]) -> [u8; 16] {
        pbkdf2::pbkdf2_hmac_array::<sha1::Sha1, 16>(secret, b"saltysalt", 1003)
    }

    /// Decrypt one `encrypted_value`. `v10` is AES-128-CBC with a fixed IV;
    /// an unprefixed value is a cookie the browser stored in the clear.
    fn decrypt(encrypted: &[u8], key: &[u8; 16]) -> Option<String> {
        use cbc::cipher::block_padding::Pkcs7;
        use cbc::cipher::{BlockDecryptMut, KeyIvInit};
        if encrypted.is_empty() {
            return None;
        }
        if !encrypted.starts_with(b"v10") {
            return String::from_utf8(encrypted.to_vec()).ok();
        }
        let mut buf = encrypted[3..].to_vec();
        if buf.is_empty() || !buf.len().is_multiple_of(16) {
            return None;
        }
        let iv = [0x20u8; 16];
        let plain = cbc::Decryptor::<aes::Aes128>::new(key.into(), &iv.into())
            .decrypt_padded_mut::<Pkcs7>(&mut buf)
            .ok()?;
        Some(decode_plaintext(plain))
    }

    /// Recent Chromium prefixes the plaintext with a 32-byte domain hash to
    /// spot stolen cookies; older versions do not. This leans on the value
    /// being printable text (a session cookie is URL-encoded ASCII): a
    /// binary-looking result means the hash is still attached, so drop it.
    /// Not a general cookie decoder — only session cookies reach here.
    fn decode_plaintext(plain: &[u8]) -> String {
        if let Ok(text) = std::str::from_utf8(plain) {
            if text.chars().all(|c| !c.is_control()) {
                return text.to_string();
            }
        }
        if plain.len() > 32 {
            if let Ok(text) = std::str::from_utf8(&plain[32..]) {
                return text.to_string();
            }
        }
        String::from_utf8_lossy(plain).into_owned()
    }

    // --- Firefox -----------------------------------------------------------

    fn firefox_session(host: &str) -> Result<Option<String>> {
        let profiles = home().join("Library/Application Support/Firefox/Profiles");
        let Ok(entries) = std::fs::read_dir(&profiles) else {
            return Ok(None);
        };
        let mut rows: Vec<(String, String, i64)> = Vec::new();
        let mut hosts: Vec<String> = Vec::new();
        for entry in entries.flatten() {
            let file = entry.path().join("cookies.sqlite");
            // A profile in use or half-written is skipped, not fatal: another
            // profile may still hold the cookie.
            let Ok(read) = read_sqlite(
                &file,
                "SELECT host, name, value, expiry FROM moz_cookies",
                |r| {
                    Ok((
                        r.get::<_, String>(0)?,
                        r.get::<_, String>(1)?,
                        r.get::<_, String>(2)?,
                        r.get::<_, i64>(3)?,
                    ))
                },
            ) else {
                continue;
            };
            for (host_key, name, value, expiry) in read {
                if host_matches(&host_key, host)
                    && SESSION_COOKIES.contains(&name.as_str())
                    && !firefox_expired(expiry)
                {
                    hosts.push(host_key);
                    rows.push((name, value, expiry));
                }
            }
        }
        let hosts_clone = hosts.clone();
        Ok(pick(rows, host, move |i| hosts_clone[i].clone()))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn expiry_treats_zero_as_a_session_cookie() {
            let now = unix_now();
            assert!(!chromium_expired(0));
            assert!(!firefox_expired(0));
            assert!(firefox_expired(now - 60));
            assert!(!firefox_expired(now + 3600));
            let now_1601_us = (now + 11_644_473_600) * 1_000_000;
            assert!(chromium_expired(now_1601_us - 60_000_000));
            assert!(!chromium_expired(now_1601_us + 3_600_000_000));
        }

        #[test]
        fn host_matching_follows_the_leading_dot() {
            assert!(host_matches(".overleaf.com", "www.overleaf.com"));
            assert!(host_matches("www.overleaf.com", "www.overleaf.com"));
            assert!(host_matches(".overleaf.com", "overleaf.com"));
            assert!(!host_matches(".overleaf.com", "notoverleaf.com"));
            assert!(!host_matches("www.overleaf.com", "overleaf.com"));
        }

        #[test]
        fn v10_round_trips_through_the_derived_key() {
            use cbc::cipher::block_padding::Pkcs7;
            use cbc::cipher::{BlockEncryptMut, KeyIvInit};
            let key = derive_key(b"a-keychain-secret");
            let iv = [0x20u8; 16];
            let value = b"s%3Aabcdef.ghijkl";
            let mut framed = b"v10".to_vec();
            let mut buf = vec![0u8; value.len() + 16];
            buf[..value.len()].copy_from_slice(value);
            let ct = cbc::Encryptor::<aes::Aes128>::new(&key.into(), &iv.into())
                .encrypt_padded_mut::<Pkcs7>(&mut buf, value.len())
                .unwrap();
            framed.extend_from_slice(ct);
            assert_eq!(decrypt(&framed, &key).unwrap(), "s%3Aabcdef.ghijkl");
        }

        #[test]
        fn a_domain_hash_prefix_is_stripped() {
            let mut plain = vec![0u8; 32];
            plain.extend_from_slice(b"s%3Areal-value");
            assert_eq!(decode_plaintext(&plain), "s%3Areal-value");
            assert_eq!(decode_plaintext(b"s%3Ano-prefix"), "s%3Ano-prefix");
        }

        #[test]
        fn unprefixed_values_are_read_as_plaintext() {
            let key = derive_key(b"x");
            assert_eq!(decrypt(b"plaincookie", &key).unwrap(), "plaincookie");
            assert!(decrypt(b"", &key).is_none());
        }

        #[test]
        fn pick_prefers_the_known_name_and_the_later_expiry() {
            let rows = vec![
                ("other".to_string(), "junk".to_string(), 999),
                ("overleaf_session2".to_string(), "old".to_string(), 10),
                ("overleaf_session2".to_string(), "new".to_string(), 20),
                ("sharelatex.sid".to_string(), "legacy".to_string(), 99),
            ];
            let hosts = vec![".overleaf.com".to_string(); rows.len()];
            let got = pick(rows, "www.overleaf.com", move |i| hosts[i].clone());
            assert_eq!(got.as_deref(), Some("overleaf_session2=new"));
        }

        #[test]
        fn pick_skips_empty_and_wrong_host() {
            let rows = vec![
                ("overleaf_session2".to_string(), String::new(), 20),
                ("overleaf_session2".to_string(), "value".to_string(), 10),
            ];
            let hosts = ["elsewhere.com".to_string(), ".overleaf.com".to_string()];
            let got = pick(rows, "www.overleaf.com", move |i| hosts[i].clone());
            assert_eq!(got.as_deref(), Some("overleaf_session2=value"));
        }
    }
}

/// Whether a browser store can be read at all here.
pub const SUPPORTED: bool = cfg!(target_os = "macos");

#[cfg(target_os = "macos")]
pub use imp::import_session;

#[cfg(not(target_os = "macos"))]
pub struct Imported {
    pub source: String,
    pub cookie: String,
}

/// No browser store is read off macOS; the paste is the only path there.
#[cfg(not(target_os = "macos"))]
pub fn import_session(_host: &str) -> crate::error::Result<Option<Imported>> {
    Ok(None)
}
