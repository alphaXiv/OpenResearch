//! Answers to an SSH host's password and passphrase prompts, kept in the OS
//! credential store (macOS Keychain, Windows Credential Manager, the Secret
//! Service through `secret-tool` elsewhere), one item per host alias.

use std::collections::BTreeMap;

use crate::error::{anyhow, Result};

const SERVICE: &str = "OpenResearch SSH";

/// Prompt text, or `passphrase <key path>`, to the answer that passed it.
pub(crate) type Answers = BTreeMap<String, String>;

pub(crate) fn load(alias: &str) -> Result<Answers> {
    match platform::read(alias)? {
        Some(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| anyhow!("The saved SSH password for {alias} is unreadable: {error}")),
        None => Ok(Answers::new()),
    }
}

/// Replaces what is saved for `alias`; no answers removes the item.
pub(crate) fn save(alias: &str, answers: &Answers) -> Result<()> {
    if answers.is_empty() {
        return platform::delete(alias);
    }
    platform::write(alias, &serde_json::to_vec(answers)?)
}

#[cfg(test)]
pub(crate) const TEST_KEYCHAIN: &str = "ORX_TEST_KEYCHAIN";

#[cfg(test)]
mod platform {
    //! A test never reaches the user's credential store. This one stays empty
    //! unless a test, alone in its process (`config::in_test_child`), sets
    //! `TEST_KEYCHAIN`, since concurrent tests share host aliases.
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    static ITEMS: Mutex<Option<HashMap<String, Vec<u8>>>> = Mutex::new(None);

    fn enabled() -> bool {
        std::env::var_os(TEST_KEYCHAIN).is_some()
    }

    pub(super) fn read(alias: &str) -> Result<Option<Vec<u8>>> {
        if !enabled() {
            return Ok(None);
        }
        Ok(ITEMS
            .lock()
            .unwrap()
            .as_ref()
            .and_then(|items| items.get(alias).cloned()))
    }

    pub(super) fn write(alias: &str, secret: &[u8]) -> Result<()> {
        if enabled() {
            ITEMS
                .lock()
                .unwrap()
                .get_or_insert_with(HashMap::new)
                .insert(alias.to_owned(), secret.to_vec());
        }
        Ok(())
    }

    pub(super) fn delete(alias: &str) -> Result<()> {
        if let Some(items) = ITEMS.lock().unwrap().as_mut() {
            items.remove(alias);
        }
        Ok(())
    }
}

#[cfg(all(not(test), target_os = "macos"))]
mod platform {
    use super::*;
    use security_framework::passwords;

    const NOT_FOUND: i32 = -25300;

    pub(super) fn read(alias: &str) -> Result<Option<Vec<u8>>> {
        match passwords::get_generic_password(SERVICE, alias) {
            Ok(secret) => Ok(Some(secret)),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(anyhow!(
                "Could not read the Keychain item for {alias}: {error}"
            )),
        }
    }

    pub(super) fn write(alias: &str, secret: &[u8]) -> Result<()> {
        passwords::set_generic_password(SERVICE, alias, secret).map_err(|error| {
            anyhow!("Could not save {alias}'s SSH password in the Keychain: {error}")
        })
    }

    pub(super) fn delete(alias: &str) -> Result<()> {
        match passwords::delete_generic_password(SERVICE, alias) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == NOT_FOUND => Ok(()),
            Err(error) => Err(anyhow!(
                "Could not remove the Keychain item for {alias}: {error}"
            )),
        }
    }
}

#[cfg(all(not(test), windows))]
mod platform {
    use super::*;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_LOCAL_MACHINE,
        CRED_TYPE_GENERIC,
    };

    fn target(alias: &str) -> Vec<u16> {
        format!("{SERVICE}: {alias}")
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect()
    }

    pub(super) fn read(alias: &str) -> Result<Option<Vec<u8>>> {
        let target = target(alias);
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            let error = unsafe { GetLastError() };
            if error == ERROR_NOT_FOUND {
                return Ok(None);
            }
            return Err(anyhow!(
                "Could not read the Credential Manager item for {alias}: {}",
                std::io::Error::from_raw_os_error(error as i32)
            ));
        }
        let secret = unsafe {
            let credential = &*credential;
            std::slice::from_raw_parts(
                credential.CredentialBlob,
                credential.CredentialBlobSize as usize,
            )
            .to_vec()
        };
        unsafe { CredFree(credential.cast()) };
        Ok(Some(secret))
    }

    pub(super) fn write(alias: &str, secret: &[u8]) -> Result<()> {
        let mut target = target(alias);
        let mut secret = secret.to_vec();
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: secret.len() as u32,
            CredentialBlob: secret.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            ..Default::default()
        };
        if unsafe { CredWriteW(&credential, 0) } == 0 {
            return Err(anyhow!(
                "Could not save {alias}'s SSH password in Credential Manager: {}",
                std::io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    pub(super) fn delete(alias: &str) -> Result<()> {
        let target = target(alias);
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } == 0 {
            let error = unsafe { GetLastError() };
            if error != ERROR_NOT_FOUND {
                return Err(anyhow!(
                    "Could not remove the Credential Manager item for {alias}: {}",
                    std::io::Error::from_raw_os_error(error as i32)
                ));
            }
        }
        Ok(())
    }
}

#[cfg(all(not(test), not(target_os = "macos"), not(windows)))]
mod platform {
    //! `secret-tool` (libsecret) reads the secret from stdin, so it never
    //! reaches a command line.
    use super::*;
    use std::io::Write as _;
    use std::process::Stdio;

    const ATTRIBUTE: &str = "openresearch-ssh";

    fn secret_tool(args: &[&str], input: Option<&[u8]>) -> Result<std::process::Output> {
        let mut child = crate::process::command("secret-tool")
            .args(args)
            .stdin(if input.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|error| {
                if error.kind() == std::io::ErrorKind::NotFound {
                    anyhow!(
                        "Saving SSH passwords needs secret-tool (libsecret-tools) and a keyring."
                    )
                } else {
                    anyhow!("Could not run secret-tool: {error}")
                }
            })?;
        if let (Some(input), Some(mut stdin)) = (input, child.stdin.take()) {
            stdin.write_all(input)?;
        }
        Ok(child.wait_with_output()?)
    }

    pub(super) fn read(alias: &str) -> Result<Option<Vec<u8>>> {
        let output = secret_tool(&["lookup", "service", ATTRIBUTE, "host", alias], None)?;
        if output.status.success() {
            return Ok(Some(output.stdout));
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        if stderr.trim().is_empty() {
            return Ok(None);
        }
        Err(anyhow!(
            "Could not read the keyring item for {alias}: {}",
            stderr.trim()
        ))
    }

    pub(super) fn write(alias: &str, secret: &[u8]) -> Result<()> {
        let label = format!("{SERVICE}: {alias}");
        let output = secret_tool(
            &[
                "store", "--label", &label, "service", ATTRIBUTE, "host", alias,
            ],
            Some(secret),
        )?;
        if !output.status.success() {
            return Err(anyhow!(
                "Could not save {alias}'s SSH password in the keyring: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            ));
        }
        Ok(())
    }

    pub(super) fn delete(alias: &str) -> Result<()> {
        let output = secret_tool(&["clear", "service", ATTRIBUTE, "host", alias], None)?;
        let stderr = String::from_utf8_lossy(&output.stderr);
        if !output.status.success() && !stderr.trim().is_empty() {
            return Err(anyhow!(
                "Could not remove the keyring item for {alias}: {}",
                stderr.trim()
            ));
        }
        Ok(())
    }
}
