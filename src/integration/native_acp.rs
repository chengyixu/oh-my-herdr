//! Bounded, task-owned ACP discovery. No prompts or model turns are sent.
use serde_json::{json, Value};
use std::io::{self, BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

pub(super) struct Discovery {
    child: Child,
    ownership: crate::platform::StatusCommandGuard,
    input: Option<ChildStdin>,
    replies: Option<Receiver<Value>>,
    reader: Option<std::thread::JoinHandle<()>>,
    deadline: Instant,
    sequence: u64,
}
impl Discovery {
    pub fn start(program: &str, args: &[&str], cwd: &std::path::Path) -> io::Result<Self> {
        Self::start_bound(program, args, cwd, 8 * 1024 * 1024, None)
    }
    pub fn start_bound(
        program: &str,
        args: &[&str],
        cwd: &std::path::Path,
        limit: usize,
        binding: Option<&(String, String)>,
    ) -> io::Result<Self> {
        let mut command = crate::noninteractive_process::command(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        if let Some((target, source)) = binding {
            let value = std::env::var_os(source).ok_or_else(|| {
                io::Error::other("native target credential reference is unavailable")
            })?;
            command.env(target, value);
        }
        super::native_process::isolate_from_pane(&mut command);
        crate::platform::configure_native_helper(&mut command)?;
        let mut child = command.spawn()?;
        let mut ownership = match crate::platform::native_helper_guard(&child) {
            Ok(guard) => guard,
            Err(error) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(error);
            }
        };
        let input = child
            .stdin
            .take()
            .ok_or_else(|| io::Error::other("native ACP input missing"))?;
        let output = child
            .stdout
            .take()
            .ok_or_else(|| io::Error::other("native ACP output missing"))?;
        let (sender, replies) = mpsc::sync_channel(4);
        let reader = std::thread::Builder::new()
            .name("native-acp-discovery".into())
            .spawn(move || {
                for line in BufReader::new(output.take(limit as u64)).lines() {
                    let Ok(line) = line else { break };
                    let Ok(value) = serde_json::from_str::<Value>(&line) else {
                        continue;
                    };
                    if value.get("method").is_none()
                        && value.get("id").is_some()
                        && sender.send(value).is_err()
                    {
                        break;
                    }
                }
            });
        match reader {
            Ok(reader) => Ok(Self {
                child,
                ownership,
                input: Some(input),
                replies: Some(replies),
                reader: Some(reader),
                deadline: Instant::now() + Duration::from_secs(15),
                sequence: 0,
            }),
            Err(error) => {
                ownership.terminate();
                let _ = child.kill();
                let _ = child.wait();
                Err(error)
            }
        }
    }
    pub fn notify(&mut self, method: &str, params: Value) -> io::Result<()> {
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| io::Error::other("native protocol closed"))?;
        serde_json::to_writer(
            &mut *input,
            &json!({"jsonrpc":"2.0","method":method,"params":params}),
        )?;
        input.write_all(b"\n")?;
        input.flush()
    }

    pub fn call(&mut self, method: &str, params: Value) -> io::Result<Value> {
        self.sequence += 1;
        let input = self
            .input
            .as_mut()
            .ok_or_else(|| io::Error::other("native ACP discovery closed"))?;
        serde_json::to_writer(
            &mut *input,
            &json!({"jsonrpc":"2.0","id":self.sequence,"method":method,"params":params}),
        )?;
        input.write_all(b"\n")?;
        input.flush()?;
        loop {
            let reply = self
                .replies
                .as_ref()
                .ok_or_else(|| io::Error::other("native ACP discovery closed"))?
                .recv_timeout(self.deadline.saturating_duration_since(Instant::now()))
                .map_err(|_| {
                    io::Error::new(
                        io::ErrorKind::TimedOut,
                        "native ACP discovery did not complete; source retained",
                    )
                })?;
            if reply["id"] != self.sequence {
                continue;
            }
            if reply.get("error").is_some() {
                return Err(io::Error::other(
                    "native ACP discovery was rejected; source retained",
                ));
            }
            return reply
                .get("result")
                .cloned()
                .ok_or_else(|| io::Error::other("native ACP reply has no result"));
        }
    }
}
impl Drop for Discovery {
    fn drop(&mut self) {
        self.input.take();
        self.ownership.terminate();
        let _ = self.child.kill();
        let _ = self.child.wait();
        // Close the consumer before joining if the bounded channel filled.
        self.replies.take();
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
