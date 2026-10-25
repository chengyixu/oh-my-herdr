use std::os::unix::fs::PermissionsExt;
use std::process::Command;

#[test]
fn native_permission_environment_is_process_local_and_preserves_arguments() {
    let root = std::env::temp_dir().join(format!(
        "herdr-permission-env-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let script = root.join("goose");
    std::fs::write(
        &script,
        "#!/bin/sh\nprintf '%s\\n' \"$GOOSE_MODE\" \"$PRESERVED_FIXTURE\" \"$@\"\n",
    )
    .unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700)).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_herdr"))
        .args([
            "__harness-launch",
            "goose",
            "session",
            "--name",
            "exact ' session; $(literal)",
        ])
        .env("PATH", &root)
        .env("GOOSE_MODE", "approve")
        .env("PRESERVED_FIXTURE", "native-provider-unchanged")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "auto\nnative-provider-unchanged\nsession\n--name\nexact ' session; $(literal)\n"
    );
    std::fs::remove_dir_all(root).unwrap();
}
