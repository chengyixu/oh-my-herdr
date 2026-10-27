//! Per-launch native editor service. Its capability is bound to one pane,
//! exact reported session and native foreground process, never a cwd guess.
use super::native_context::NativeContextRegistration;
use serde_json::{json, Value};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct NativeSourceHandoff {
    pub(crate) directory: PathBuf,
    pub(crate) helper_pid: u32,
    pub(crate) input: crate::pane::native_input::NativeInputEvidence,
    pub(crate) baseline: Option<crate::pane::native_input::NativeInputEvidence>,
}

pub(crate) struct ManagedSourceLease {
    directory: PathBuf,
    configuration: Value,
    child: Option<Child>,
    helper_pid: u32,
    ownership: Option<crate::platform::StatusCommandGuard>,
    owned: bool,
}
impl Drop for ManagedSourceLease {
    fn drop(&mut self) {
        if !self.owned {
            return;
        }
        if self.child.is_none() {
            use std::os::unix::net::UnixStream;
            if let Ok(mut stream) = UnixStream::connect(self.directory.join("native-context.sock"))
            {
                let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(100)));
                let request = json!({"token":self.configuration["token"],"action":"shutdown"});
                let _ = writeln!(stream, "{request}");
            }
            return;
        }
        if let Some(guard) = &mut self.ownership {
            guard.terminate();
        }
        if let Some(child) = &mut self.child {
            let _ = child.kill();
            let _ = child.wait();
        }
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}
impl ManagedSourceLease {
    pub(crate) fn handoff_state(
        &self,
        input: crate::pane::native_input::NativeInputEvidence,
        baseline: Option<crate::pane::native_input::NativeInputEvidence>,
    ) -> NativeSourceHandoff {
        NativeSourceHandoff {
            directory: self.directory.clone(),
            helper_pid: self.helper_pid,
            input,
            baseline,
        }
    }
    pub(crate) fn release_for_handoff(&mut self) {
        if let Some(guard) = &mut self.ownership {
            guard.release_for_handoff();
        }
        self.owned = false;
    }
    pub(crate) fn adopt_handoff(state: &NativeSourceHandoff) -> io::Result<Self> {
        crate::platform::validate_private_context_endpoint(
            &state.directory.join("native-context.sock"),
        )?;
        let configuration: Value =
            serde_json::from_slice(&std::fs::read(state.directory.join("lease.json"))?)?;
        Ok(Self {
            directory: state.directory.clone(),
            configuration,
            child: None,
            helper_pid: state.helper_pid,
            ownership: None,
            owned: false,
        })
    }
    pub(crate) fn assume_handoff_ownership(&mut self) -> io::Result<()> {
        self.owned = true;
        self.configuration["manager_pid"] = json!(std::process::id());
        self.configuration["binary"] = serde_json::to_value(std::env::current_exe()?)?;
        write_config(&self.directory.join("lease.json"), &self.configuration)?;
        Ok(())
    }
    pub(crate) fn create(cwd: &Path) -> io::Result<Self> {
        let (directory, _) = super::native_process::private_directory()?;
        let result = (|| {
            let script = directory.join("source.py");
            std::fs::write(&script, include_bytes!("assets/native-context-service.py"))?;
            let lease = directory.join("lease.json");
            let mut entropy = [0u8; 32];
            std::fs::File::open("/dev/urandom")?.read_exact(&mut entropy)?;
            let token = entropy
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>();
            let config = json!({"endpoint":directory.join("native-context.sock"),"token":token,
                "api_endpoint":crate::api::socket_path(),"binary":std::env::current_exe()?,"manager_pid":std::process::id(),"cwd":cwd,"context":null});
            write_config(&lease, &config)?;
            let editor_path = directory.join("editor.sh");
            std::fs::write(
                &editor_path,
                format!(
                    "#!/bin/sh\nexec python3 {} editor --lease {} \"$@\" 2>>{}\n",
                    word(&script.to_string_lossy()),
                    word(&lease.to_string_lossy()),
                    word(&directory.join("service.log").to_string_lossy())
                ),
            )?;
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&editor_path, std::fs::Permissions::from_mode(0o700))?;
            let editor = editor_path.to_string_lossy();
            let launcher=format!("#!/bin/sh\nexport OHMYHERDR_NATIVE_EDITOR_FALLBACK=\"${{VISUAL:-${{EDITOR:-vi}}}}\"\nexport VISUAL={}\nexport EDITOR=\"$VISUAL\"\nexec \"$@\"\n",word(&editor));
            std::fs::write(directory.join("launch.sh"), launcher)?;
            let mut command = crate::noninteractive_process::command("python3");
            command
                .arg(script)
                .arg("server")
                .arg("--lease")
                .arg(&lease)
                .current_dir(cwd)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null());
            super::native_process::isolate_from_pane(&mut command);
            crate::platform::configure_native_helper(&mut command)?;
            let mut child = command.spawn()?;
            let ownership = match crate::platform::native_helper_guard(&child) {
                Ok(guard) => guard,
                Err(error) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            };
            Ok(Self {
                directory: directory.clone(),
                configuration: config,
                helper_pid: child.id(),
                child: Some(child),
                ownership: Some(ownership),
                owned: true,
            })
        })();
        if result.is_err() {
            let _ = std::fs::remove_dir_all(directory);
        }
        result
    }
    pub(crate) fn launch_argv(&self, argv: &[String]) -> Vec<String> {
        let mut result = vec![
            "/bin/sh".into(),
            self.directory
                .join("launch.sh")
                .to_string_lossy()
                .into_owned(),
        ];
        result.extend_from_slice(argv);
        result
    }
    pub(crate) fn activate(
        &mut self,
        agent: &str,
        pane: &str,
        id: &str,
        path: &Path,
        process_id: u32,
        baseline: Option<crate::pane::native_input::NativeInputEvidence>,
    ) -> io::Result<NativeContextRegistration> {
        let registration = NativeContextRegistration {
            agent: agent.into(),
            source: format!("herdr:{agent}"),
            pane_id: pane.into(),
            session_id: id.into(),
            session_path: path.to_string_lossy().into(),
            process_id,
            seq: 1,
            version: 1,
            endpoint: self.configuration["endpoint"]
                .as_str()
                .ok_or_else(|| io::Error::other("source endpoint missing"))?
                .into(),
            token: self.configuration["token"]
                .as_str()
                .ok_or_else(|| io::Error::other("source capability missing"))?
                .into(),
        };
        self.configuration["baseline"] = serde_json::to_value(baseline)?;
        self.configuration["binary"] = serde_json::to_value(std::env::current_exe()?)?;
        self.configuration["context"] = serde_json::to_value(&registration)?;
        write_config(&self.directory.join("lease.json"), &self.configuration)?;
        Ok(registration)
    }
}
fn word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn write_config(path: &Path, config: &Value) -> io::Result<()> {
    let pending = path.with_extension("pending");
    let mut file = crate::platform::create_private_state_file(&pending)?;
    serde_json::to_writer(&mut file, config)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(pending, path)
}
