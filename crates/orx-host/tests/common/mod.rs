#![allow(dead_code)]
use orx_host::{nonce, platform, publish, read_json, wrapper};
use orx_host_proto::{LaunchIntent, ProcessIdentity};
use std::fs;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

pub struct Directory(pub PathBuf);

pub struct Group(pub Child);
impl Drop for Group {
    fn drop(&mut self) {
        unsafe {
            libc::kill(-(self.0.id() as i32), libc::SIGKILL);
        }
        let _ = self.0.wait();
    }
}

impl Directory {
    pub fn new() -> Self {
        let path = PathBuf::from(format!(
            "/tmp/orx-host-{}-{}",
            std::process::id(),
            &nonce().unwrap()[..12]
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Directory {
    fn drop(&mut self) {
        match fs::remove_dir_all(&self.0) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            result => result.unwrap(),
        }
    }
}

pub fn until(timeout: Duration, mut condition: impl FnMut() -> bool) {
    let start = Instant::now();
    while !condition() {
        assert!(start.elapsed() < timeout, "timed out after {timeout:?}");
        thread::sleep(Duration::from_millis(10));
    }
}

pub struct Run {
    pub directory: Directory,
    pub children: Vec<Child>,
    pub extra: Vec<platform::Process>,
}
impl Run {
    pub fn new(script: &str) -> Self {
        let directory = Directory::new();
        fs::write(directory.0.join("run.sh"), script).unwrap();
        publish(
            &directory.0.join("launch.json"),
            &LaunchIntent {
                run: "test".into(),
                digest: orx_host::sha(script.as_bytes()),
                run_sh: script.into(),
                env: Default::default(),
                source: None,
                cancel_sh: None,
            },
        )
        .unwrap();
        Self {
            directory,
            children: vec![],
            extra: vec![],
        }
    }
    pub fn spawn(&mut self) {
        self.children.push(
            Command::new(env!("CARGO_BIN_EXE_orx-host"))
                .env("ORX_TEST_FAST_STOPS", "1")
                .arg("wrap")
                .arg(&self.directory.0)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
    }
    pub fn ready(&self) {
        until(Duration::from_secs(5), || {
            fs::read_to_string(self.directory.0.join("started"))
                .is_ok_and(|tag| tag.ends_with('\n'))
                && read_json::<ProcessIdentity>(&self.directory.0.join("wrapper.json")).is_ok()
        });
    }
    pub fn wait(&mut self) {
        for child in &mut self.children {
            until(Duration::from_secs(15), || {
                child.try_wait().unwrap().is_some()
            });
            assert!(child.wait().unwrap().success());
        }
    }
}
impl Drop for Run {
    fn drop(&mut self) {
        let _ = wrapper::cancel(&self.directory.0);
        let _ = wrapper::cleanup(&self.directory.0);
        let end = Instant::now() + Duration::from_secs(14);
        for child in &mut self.children {
            while child.try_wait().ok().flatten().is_none() && Instant::now() < end {
                thread::sleep(Duration::from_millis(20));
            }
            if child.try_wait().ok().flatten().is_none() {
                let _ = child.kill();
            }
            let _ = child.wait();
        }
        for process in &self.extra {
            let _ = platform::signal(process, libc::SIGKILL);
        }
    }
}
