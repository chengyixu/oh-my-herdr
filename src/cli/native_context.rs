//! Internal bounded exporter used only by a pane's native editor bridge.
use std::io;
use std::path::Path;

pub(super) fn run(args: &[String]) -> io::Result<i32> {
    let [operation, agent, path, id, cwd] = args else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid native context request",
        ));
    };
    if operation != "snapshot" {
        return Err(io::Error::other("unknown native context operation"));
    }
    let snapshot = match crate::integration::session_history::export_snapshot(
        agent,
        Path::new(path),
        id,
        Path::new(cwd),
    ) {
        Ok(snapshot) => snapshot,
        Err(error) => {
            println!("{}", serde_json::json!({"error":error.to_string()}));
            return Ok(1);
        }
    };
    println!("{}", serde_json::to_string(&snapshot)?);
    Ok(0)
}
