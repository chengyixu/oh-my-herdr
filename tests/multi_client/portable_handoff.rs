use super::*;
use serde_json::json;

fn endpoint(stream: &mut UnixStream, boot: &str, method: &str, params: Value) -> Value {
    let response = support::client_shell_endpoint_request(
        stream,
        boot,
        &json!({
            "id": method, "method": method, "params": params,
        }),
    );
    assert!(response.get("error").is_none(), "{method}: {response}");
    response["result"].clone()
}

fn harness_pid(stream: &mut UnixStream, boot: &str, pane: &str) -> u32 {
    let read = endpoint(
        stream,
        boot,
        "pane.read",
        json!({"pane_id":pane,"source":"recent"}),
    );
    let text = read["read"]["text"].as_str().unwrap();
    text.split("transcript-pid=")
        .last()
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap()
}

fn wait_for_capture(path: &Path, needle: &str) {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let text = fs::read_to_string(path).unwrap_or_default();
        if text.contains(needle) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "missing {needle} from {}: {text:?}",
            path.display()
        );
        thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn client_endpoint_portable_handoff_delivers_context_and_preserves_exact_sessions() {
    use std::process::Command;
    let _lock = test_lock();
    let base = unique_test_dir();
    let config = base.join("config");
    let runtime = base.join("runtime");
    let api = runtime.join("herdr.sock");
    let clients = runtime.join("herdr-client.sock");
    let bin = base.join("bin");
    let captures = base.join("captures");
    fs::create_dir_all(&bin).unwrap();
    fs::create_dir_all(&captures).unwrap();
    let compiled = Command::new("cc")
        .args(["-Wall", "-Wextra", "-Werror"])
        .arg(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/interactive-harness.c"))
        .arg("-o")
        .arg(bin.join("pi"))
        .output()
        .unwrap();
    assert!(
        compiled.status.success(),
        "{}",
        String::from_utf8_lossy(&compiled.stderr)
    );
    fs::copy(bin.join("pi"), bin.join("droid")).unwrap();
    // Synthetic rules exercise the detection engine, independent of vendor screens.
    let product = if option_env!("OHMYHERDR_BUILD") == Some("1") {
        "ohmyherdr"
    } else if cfg!(debug_assertions) {
        "herdr-dev"
    } else {
        "herdr"
    };
    let detection = config.join(product).join("agent-detection");
    fs::create_dir_all(&detection).unwrap();
    for harness in ["pi", "droid"] {
        fs::write(detection.join(format!("{harness}.toml")), format!(
            "id = \"{harness}\"\nversion = \"test\"\n[[rules]]\nid = \"test-ready\"\nstate = \"idle\"\npriority = 1\nregion = \"whole_recent\"\ncontains = [\"PORTABLE_TEST_READY\"]\n"
        )).unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let capture_dir = captures.to_str().unwrap();
    let server = spawn_server_with_env(
        &config,
        &runtime,
        &api,
        &[("PATH", &path), ("HERDR_TEST_CAPTURE_DIR", capture_dir)],
    );
    wait_for_socket(&clients, Duration::from_secs(10));
    let root = create_pane(&api, "portable-handoff");
    let mut stream = UnixStream::connect(&clients).unwrap();
    let welcome =
        support::client_shell_handshake_welcome(&mut stream, CURRENT_PROTOCOL, 120, 40).unwrap();
    assert!(welcome["error"].is_null(), "{welcome}");
    let boot = support::client_shell_boot_id(&mut stream);
    for method in [
        "agent.harness.history",
        "agent.harness.context",
        "pane.read",
        "agent.spawn",
        "agent.ready.wait",
        "agent.prompt",
        "agent.roster.list",
        "agent.harness.set",
        "agent.harness.commit",
        "pane.focus",
    ] {
        assert!(
            welcome["methods"]
                .as_array()
                .unwrap()
                .iter()
                .any(|name| name == method),
            "missing {method} from welcome"
        );
    }
    for harness in ["pi", "droid"] {
        endpoint(
            &mut stream,
            &boot,
            "integration.harness.set",
            json!({"target":harness,"active":true}),
        );
    }

    let pane = api_request(
        &api,
        &json!({"id":"root","method":"pane.get","params":{"pane_id":root}}).to_string(),
    )["result"]
        .clone();
    let tab = pane["pane"]["tab_id"].as_str().unwrap().to_owned();
    pane_input(&api, &root, bin.join("pi").to_str().unwrap());
    assert!(pane_contains(
        &api,
        &root,
        "PORTABLE_TEST_READY",
        Duration::from_secs(5)
    ));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let info = api_request(
            &api,
            &json!({"id":"manual-agent","method":"agent.get","params":{"target":root}}).to_string(),
        );
        if info["result"]["agent"]["agent"] == "pi"
            && info["result"]["agent"]["agent_status"] == "idle"
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "manual native process was not detected: {info}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let prepared = endpoint(
        &mut stream,
        &boot,
        "agent.harness.prepare",
        json!({"target":root}),
    );
    let source_id = prepared["instance_id"].as_str().unwrap().to_owned();
    let roster = endpoint(&mut stream, &boot, "agent.roster.list", json!({}));
    let role = roster["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["instance_id"] == source_id)
        .unwrap()["role"]
        .as_str()
        .unwrap()
        .to_owned();
    endpoint(
        &mut stream,
        &boot,
        "agent.profile.edit",
        json!({"role":role,"settings":{"role":role,"native_cwd":base},"documents":{"AGENTS.md":"PERSISTENT-DOCUMENT-CONTEXT"}}),
    );
    let source_pane = root.clone();
    endpoint(
        &mut stream,
        &boot,
        "agent.ready.wait",
        json!({"target":source_pane,"timeout_ms":10000}),
    );
    let source_pid = harness_pid(&mut stream, &boot, &source_pane);
    let sibling = endpoint(
        &mut stream,
        &boot,
        "agent.spawn",
        json!({"role":role,"kind":"pi","tab_id":tab,"cwd_mode":"agent","timeout_ms":10000}),
    );
    let sibling_pane = sibling["spawned"]["pane_id"].as_str().unwrap().to_owned();
    endpoint(
        &mut stream,
        &boot,
        "agent.ready.wait",
        json!({"target":sibling_pane,"timeout_ms":10000}),
    );
    let sibling_pid = harness_pid(&mut stream, &boot, &sibling_pane);

    let context = endpoint(
        &mut stream,
        &boot,
        "agent.harness.context",
        json!({"instance_id":source_id}),
    );
    assert!(context["documents"]
        .to_string()
        .contains("PERSISTENT-DOCUMENT-CONTEXT"));
    let transcript = endpoint(
        &mut stream,
        &boot,
        "pane.read",
        json!({"pane_id":source_pane,"source":"recent","lines":200}),
    );
    let target = endpoint(
        &mut stream,
        &boot,
        "agent.spawn",
        json!({"role":role,"kind":"droid","tab_id":tab,"cwd_mode":"agent","timeout_ms":10000}),
    );
    let target_pane = target["spawned"]["pane_id"].as_str().unwrap().to_owned();
    let ready = endpoint(
        &mut stream,
        &boot,
        "agent.ready.wait",
        json!({"target":target_pane,"timeout_ms":10000}),
    );
    assert_eq!(ready["agent"]["interactive_ready"], true);
    let target_pid = harness_pid(&mut stream, &boot, &target_pane);
    let blocked = api_request(&api, &json!({"id":"block-target","method":"pane.report_agent","params":{"pane_id":target_pane,"agent":"droid","state":"blocked","source":"test:startup"}}).to_string());
    assert!(blocked.get("error").is_none(), "{blocked}");
    let readiness = support::client_shell_endpoint_request(
        &mut stream,
        &boot,
        &json!({"id":"blocked-ready","method":"agent.ready.wait","params":{"target":target_pane,"timeout_ms":150}}),
    );
    assert_eq!(
        readiness["error"]["code"], "timeout",
        "a native confirmation is not ready for handoff input: {readiness}"
    );
    let cleared = api_request(&api, &json!({"id":"clear-target","method":"pane.clear_agent_authority","params":{"pane_id":target_pane,"source":"test:startup"}}).to_string());
    assert!(cleared.get("error").is_none(), "{cleared}");
    endpoint(
        &mut stream,
        &boot,
        "agent.ready.wait",
        json!({"target":target_pane,"timeout_ms":10000}),
    );
    let prompt = format!(
        "PORTABLE-DELIVERY\n{}\n{}",
        context["documents"],
        transcript["read"]["text"].as_str().unwrap()
    );
    let roster = endpoint(&mut stream, &boot, "agent.roster.list", json!({}));
    let target_id = roster["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["last_pane"] == target_pane)
        .unwrap()["instance_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let wrong_receiver = support::client_shell_endpoint_request(
        &mut stream,
        &boot,
        &json!({"id":"wrong-receiver","method":"agent.harness.prompt","params":{"instance_id":source_id,"pane_id":target_pane,"harness":"droid","text":"WRONG-RECEIVER-CONTEXT"}}),
    );
    assert_eq!(
        wrong_receiver["error"]["code"], "agent_switch_stale",
        "{wrong_receiver}"
    );
    endpoint(
        &mut stream,
        &boot,
        "agent.harness.prompt",
        json!({"instance_id":target_id,"pane_id":target_pane,"harness":"droid","text":prompt}),
    );
    let target_capture = captures.join(format!("{target_pid}.input"));
    wait_for_capture(&target_capture, "PORTABLE-DELIVERY");
    let delivered = fs::read_to_string(&target_capture).unwrap();
    assert!(delivered.contains("PERSISTENT-DOCUMENT-CONTEXT"));
    assert!(delivered.contains(&format!("transcript-pid={source_pid}")));
    assert!(!delivered.contains("WRONG-RECEIVER-CONTEXT"));
    endpoint(
        &mut stream,
        &boot,
        "agent.harness.set",
        json!({"instance_id":target_id,"harness":"droid"}),
    );
    let commit_params = json!({"source_instance_id":source_id,"source_pane_id":source_pane,"target_instance_id":target_id,"target_pane_id":target_pane,"label":"Previous pi (still running)"});
    let mut stale = commit_params.clone();
    stale["source_pane_id"] = json!(sibling_pane);
    let rejected = support::client_shell_endpoint_request(
        &mut stream,
        &boot,
        &json!({"id":"stale","method":"agent.harness.commit","params":stale}),
    );
    assert_eq!(rejected["error"]["code"], "agent_switch_stale");
    let registry_dir = config.join(product);
    let lock = registry_dir.join(".agents.lock");
    let saved_lock = registry_dir.join(".agents.lock-saved");
    fs::rename(&lock, &saved_lock).unwrap();
    fs::create_dir(&lock).unwrap();
    let persist_rejected = support::client_shell_endpoint_request(
        &mut stream,
        &boot,
        &json!({"id":"persist-rejected","method":"agent.harness.commit","params":commit_params}),
    );
    fs::remove_dir(&lock).unwrap();
    fs::rename(&saved_lock, &lock).unwrap();
    assert_eq!(
        persist_rejected["error"]["code"], "agent_profile_persist_failed",
        "{persist_rejected}"
    );
    let original = api_request(
        &api,
        &json!({"id":"original","method":"pane.get","params":{"pane_id":source_pane}}).to_string(),
    );
    assert_eq!(original["result"]["pane"]["tab_id"], tab);
    let committed = endpoint(&mut stream, &boot, "agent.harness.commit", commit_params);
    assert_eq!(committed["pane"]["pane_id"], target_pane);
    let snapshot = support::client_shell_snapshot_until(&mut stream, |s| {
        s["focused_pane_id"] == target_pane
            && s["panes"].as_array().is_some_and(|panes| {
                panes
                    .iter()
                    .any(|p| p["pane_id"] == source_pane && p["tab_id"] != tab)
            })
    });
    assert_eq!(snapshot["focused_tab_id"], tab);
    for pid in [source_pid, sibling_pid, target_pid] {
        assert_eq!(
            unsafe { libc::kill(pid as libc::pid_t, 0) },
            0,
            "session {pid} was lost"
        );
    }
    let history = endpoint(
        &mut stream,
        &boot,
        "agent.harness.history",
        json!({"instance_id":target_id}),
    );
    assert_eq!(history["instance_ids"], json!([source_id]));
    endpoint(
        &mut stream,
        &boot,
        "agent.ready.wait",
        json!({"target":source_pane,"timeout_ms":10000}),
    );
    endpoint(
        &mut stream,
        &boot,
        "agent.prompt",
        json!({"target":source_pane,"text":"RETURN-DELIVERY"}),
    );
    wait_for_capture(
        &captures.join(format!("{source_pid}.input")),
        "RETURN-DELIVERY",
    );
    let sibling_capture =
        fs::read_to_string(captures.join(format!("{sibling_pid}.input"))).unwrap_or_default();
    assert!(!sibling_capture.contains("RETURN-DELIVERY"));
    endpoint(
        &mut stream,
        &boot,
        "pane.focus",
        json!({"pane_id":source_pane}),
    );
    let focused =
        support::client_shell_snapshot_until(&mut stream, |s| s["focused_pane_id"] == source_pane);
    assert_eq!(focused["focused_tab_id"], committed["previous_tab"]);
    let registry: Value =
        serde_json::from_slice(&fs::read(config.join(product).join("agents.json")).unwrap())
            .unwrap();
    assert_eq!(registry["harness_handoffs"][&target_id], source_id);
    cleanup(server, base);
}
