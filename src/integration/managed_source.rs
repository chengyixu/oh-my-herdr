#![allow(dead_code)]

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{self, Receiver};

use serde_json::{json, Value};

use crate::pane::native_input::NativeInputEvidence;

pub(crate) struct ManagedSourceLease {
    pub directory: PathBuf,
    pub lease_file: PathBuf,
    pub configuration: Value,
    pub baseline: NativeInputEvidence,
    pub registration: Option<crate::integration::native_context::NativeContextRegistration>,
    ready_probe: Option<Receiver<Result<(), String>>>,
    pub ready: bool,
    ready_error: Option<String>,
    _process: Child,
}

impl Drop for ManagedSourceLease {
    fn drop(&mut self) {
        let _ = self._process.kill();
        let _ = self._process.wait();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl ManagedSourceLease {
    pub fn create(
        agent: &str,
        pane_id: &str,
        cwd: &Path,
        binary_path: &Path,
        api_endpoint: &Path,
        baseline: NativeInputEvidence,
    ) -> io::Result<Self> {
        let (directory, _) = super::native_process::private_directory()?;
        let source_script = directory.join("source.py");
        let script_content = include_str!("../integration/assets/native-context-service.py");
        std::fs::write(&source_script, script_content)?;

        let sock_path = directory.join("native-context.sock");
        let lease_file = directory.join("lease.json");
        let token = format!("{:x}", rand_u128());

        let context = json!({
            "agent": agent,
            "pane_id": pane_id,
            "session_id": "",
            "session_path": "",
            "process_id": 0,
        });

        let config = json!({
            "endpoint": sock_path.to_string_lossy(),
            "token": token,
            "api_endpoint": api_endpoint.to_string_lossy(),
            "binary": binary_path.to_string_lossy(),
            "context": context,
        });

        write_config(&lease_file, &config)?;

        let editor_script = directory.join("editor.sh");
        let editor_content = format!(
            "#!/bin/sh\nexec python3 '{}' editor --lease '{}' \"$@\"\n",
            source_script.display(),
            lease_file.display()
        );
        std::fs::write(&editor_script, editor_content)?;
        set_executable(&editor_script)?;

        let launch_script = directory.join("launch.sh");
        let launch_content = format!(
            "#!/bin/sh\nexport EDITOR=\"/bin/sh {}\"\nexport VISUAL=\"$EDITOR\"\nexec \"$@\"\n",
            shell_word(&editor_script.to_string_lossy())
        );
        std::fs::write(&launch_script, launch_content)?;
        set_executable(&launch_script)?;

        let mut cmd = crate::noninteractive_process::command("python3");
        cmd.args([
            source_script.to_str().unwrap(),
            "server",
            "--lease",
            lease_file.to_str().unwrap(),
        ])
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

        super::native_process::isolate_from_pane(&mut cmd);
        let child = cmd.spawn()?;

        Ok(Self {
            directory,
            lease_file,
            configuration: config,
            baseline,
            registration: None,
            ready_probe: None,
            ready: false,
            ready_error: None,
            _process: child,
        })
    }

    pub fn launch_argv(&self, original_argv: &[String]) -> Vec<String> {
        let mut argv = vec![
            "/bin/sh".to_string(),
            self.directory
                .join("launch.sh")
                .to_string_lossy()
                .into_owned(),
        ];
        argv.extend_from_slice(original_argv);
        argv
    }

    pub fn poll_ready(
        &mut self,
        registration: &Option<crate::integration::native_context::NativeContextRegistration>,
    ) -> io::Result<bool> {
        if self.ready {
            return Ok(true);
        }
        if let Some(err) = &self.ready_error {
            return Err(io::Error::other(err.clone()));
        }

        if self.ready_probe.is_none() {
            if let Some(reg) = registration {
                self.registration = Some(reg.clone());
                self.configuration["context"] = json!({
                    "agent": reg.agent,
                    "pane_id": reg.pane_id,
                    "session_id": reg.session_id,
                    "session_path": reg.session_path,
                    "process_id": reg.process_id,
                });
                write_config(&self.lease_file, &self.configuration)?;

                let (tx, rx) = mpsc::channel();
                let sock_path = self.directory.join("native-context.sock");
                let token = self.configuration["token"]
                    .as_str()
                    .unwrap_or("")
                    .to_string();

                std::thread::Builder::new()
                    .name("native-target-ready".into())
                    .spawn(move || {
                        let res = probe_ready(&sock_path, &token);
                        let _ = tx.send(res);
                    })
                    .map_err(io::Error::other)?;
                self.ready_probe = Some(rx);
            }
        }

        if let Some(rx) = &self.ready_probe {
            match rx.try_recv() {
                Ok(Ok(())) => {
                    self.ready = true;
                    self.ready_probe = None;
                    Ok(true)
                }
                Ok(Err(err)) => {
                    self.ready_error = Some(err.clone());
                    Err(io::Error::other(err))
                }
                Err(mpsc::TryRecvError::Empty) => Ok(false),
                Err(mpsc::TryRecvError::Disconnected) => {
                    let err = "native target readiness acknowledgement failed".to_string();
                    self.ready_error = Some(err.clone());
                    Err(io::Error::other(err))
                }
            }
        } else {
            Ok(false)
        }
    }
}

fn probe_ready(sock_path: &Path, token: &str) -> Result<(), String> {
    use std::io::{Read, Write};
    use std::os::unix::net::UnixStream;

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    while std::time::Instant::now() < deadline {
        if let Ok(mut stream) = UnixStream::connect(sock_path) {
            let req = json!({
                "token": token,
                "action": "ready"
            });
            let mut bytes = serde_json::to_vec(&req).map_err(|e| e.to_string())?;
            bytes.push(b'\n');
            if stream.write_all(&bytes).is_ok() {
                let mut resp = Vec::new();
                let mut buf = [0u8; 1024];
                if let Ok(n) = stream.read(&mut buf) {
                    resp.extend_from_slice(&buf[..n]);
                    if let Ok(val) = serde_json::from_slice::<Value>(&resp) {
                        if val.get("ok").and_then(Value::as_bool) == Some(true) {
                            return Ok(());
                        }
                    }
                }
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    Err("native target readiness acknowledgement failed".into())
}

fn write_config(path: &Path, config: &Value) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(config)?;
    std::fs::write(path, bytes)
}

fn shell_word(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

fn set_executable(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

fn rand_u128() -> u128 {
    use std::time::SystemTime;
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    nanos ^ (std::process::id() as u128)
}
