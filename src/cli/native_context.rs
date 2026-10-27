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
    let result = match operation.as_str() {
        "snapshot" => crate::integration::session_history::export_snapshot(
            agent,
            Path::new(path),
            id,
            Path::new(cwd),
        )
        .and_then(|s| serde_json::to_value(s).map_err(io::Error::other)),
        "queue" if agent == "codex" => {
            crate::integration::session_history::codex_native::queue_empty(Path::new(cwd), id)
                .map(|empty| serde_json::json!({"empty":empty}))
        }
        _ => return Err(io::Error::other("unknown native context operation")),
    };
    match result {
        Ok(value) => {
            println!("{value}");
            Ok(0)
        }
        Err(error) => {
            println!("{}", serde_json::json!({"error":error.to_string()}));
            Ok(1)
        }
    }
}
