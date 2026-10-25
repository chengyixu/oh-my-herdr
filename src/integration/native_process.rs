//! Bounded native CLI operations scoped to an owned helper process group.
use std::io;
use std::path::Path;
use std::process::Stdio;
use std::time::{Duration, Instant};

const PANE_ENV: [&str; 5] = [
    "HERDR_PANE_ID",
    "HERDR_TAB_ID",
    "HERDR_WORKSPACE_ID",
    "HERDR_SOCKET_PATH",
    "HERDR_CLIENT_SOCKET_PATH",
];

/// Remove pane ownership so a helper can never claim the source pane through a hook.
pub(super) fn isolate_from_pane(command: &mut std::process::Command) {
    for name in PANE_ENV {
        command.env_remove(name);
    }
}

/// Create a fresh owner-only directory for bounded helper output.
pub(super) fn private_directory() -> io::Result<(std::path::PathBuf, ())> {
    let base = crate::platform::remote_private_temp_base();
    for attempt in 0..100u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.subsec_nanos())
            .unwrap_or_default();
        let path = base.join(format!(
            "herdr-native-{}-{nanos:x}-{attempt}",
            std::process::id()
        ));
        match crate::platform::create_remote_private_dir(&path) {
            Ok(()) => return Ok((path, ())),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::AlreadyExists,
        "failed to create a private native helper directory",
    ))
}

pub(super) fn capture(
    program: &str,
    args: &[&str],
    cwd: &Path,
    limit: usize,
) -> io::Result<Vec<u8>> {
    let (directory, _) = private_directory()?;
    let result = (|| -> io::Result<Vec<u8>> {
        let output = directory.join("stdout");
        let errors = directory.join("stderr");
        let stdout = crate::platform::create_private_state_file(&output)?;
        let stderr = crate::platform::create_private_state_file(&errors)?;
        let mut command = crate::noninteractive_process::command(program);
        command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::null())
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .env("NO_COLOR", "1");
        isolate_from_pane(&mut command);
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
        let deadline = Instant::now() + Duration::from_secs(15);
        let status = loop {
            if Instant::now() >= deadline
                || std::fs::metadata(&output)?.len() > limit as u64
                || std::fs::metadata(&errors)?.len() > 64 * 1024
            {
                ownership.terminate();
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "native CLI timed out or exceeded its output bound; source retained",
                ));
            }
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(error) => {
                    ownership.terminate();
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(error);
                }
            }
        };
        ownership.terminate();
        if !status.success() {
            return Err(io::Error::other(format!(
                "native {program} operation was rejected; source retained"
            )));
        }
        match crate::platform::read_limited_reader(std::fs::File::open(output)?, limit)? {
            crate::platform::LimitedRead::Complete(bytes) => Ok(bytes),
            crate::platform::LimitedRead::Empty => Ok(Vec::new()),
            crate::platform::LimitedRead::Oversized => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "native CLI output exceeded its bound; nothing was truncated",
            )),
        }
    })();
    let _ = std::fs::remove_dir_all(directory);
    result
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn capture_preserves_output_larger_than_a_pipe_and_refuses_overflow() {
        let (root, _) = private_directory().unwrap();
        let bytes = capture(
            "python3",
            &["-c", "import sys;sys.stdout.write('Q'*200000)"],
            &root,
            256 * 1024,
        )
        .unwrap();
        assert_eq!(bytes, vec![b'Q'; 200000]);
        assert!(capture(
            "python3",
            &["-c", "import sys;sys.stdout.write('Q'*300000)"],
            &root,
            65536
        )
        .is_err());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn capture_cleans_only_its_owned_background_descendant() {
        let (root, _) = private_directory().unwrap();
        let root = root.canonicalize().unwrap();
        let script = "import subprocess,sys,pathlib;p=subprocess.Popen([sys.executable,'-c','import time;time.sleep(30)']);pathlib.Path('descendant.pid').write_text(str(p.pid));print('captured')";
        assert_eq!(
            capture("python3", &["-c", script], &root, 65536).unwrap(),
            b"captured\n"
        );
        let pid = std::fs::read_to_string(root.join("descendant.pid"))
            .unwrap()
            .parse::<u32>()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while crate::platform::process_cwd(pid).as_ref() == Some(&root) && Instant::now() < deadline
        {
            std::thread::sleep(Duration::from_millis(10));
        }
        let still_running = crate::platform::process_cwd(pid).as_ref() == Some(&root);
        if still_running {
            crate::platform::signal_processes(&[pid], crate::platform::Signal::Kill);
        }
        std::fs::remove_dir_all(root).unwrap();
        assert!(
            !still_running,
            "native helper left an owned descendant running"
        );
    }
}
