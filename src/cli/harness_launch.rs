//! Process-local native harness environment; never edits vendor configuration.
use std::io;

pub(super) fn run(args: &[String]) -> io::Result<i32> {
    let (id, args) = args
        .split_first()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing native harness"))?;
    let harness = crate::harness::find(id)
        .filter(|h| !h.launch_environment().is_empty())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "unknown environment-scoped harness",
            )
        })?;
    let mut command = std::process::Command::new(harness.executable);
    command
        .args(args)
        .envs(harness.launch_environment().iter().copied());
    crate::platform::run_foreground_native_command(&mut command)
}
