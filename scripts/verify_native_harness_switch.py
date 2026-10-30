"""Drive same-pane replacement via the actual generation-1 client endpoint."""

import argparse, datetime, hashlib, json, os, pathlib, shlex, shutil, socket, struct, subprocess, threading, time, uuid, tempfile
import sys

parser = argparse.ArgumentParser(
    description="Qualify same-pane Pi-to-Codex native history exchange without a model turn"
)
parser.add_argument("--binary", type=pathlib.Path, required=True)
parser.add_argument("--output", type=pathlib.Path, required=True)
parser.add_argument(
    "--verify-model-recall",
    action="store_true",
    help="Send one harmless native-model recall prompt after import",
)
parser.add_argument(
    "--fail-target-launch",
    action="store_true",
    help="Use a task-owned failing target executable to qualify rollback",
)
parser.add_argument(
    "--empty-source",
    action="store_true",
    help="Qualify an instruction-only Pi session before its first user message",
)
parser.add_argument(
    "--direct-source",
    action="store_true",
    help="Start the named source without a pre-existing saved profile",
)
parser.add_argument(
    "--handoff-from", type=pathlib.Path,
    help="Start the pane on this older server binary, live-hand it off to --binary, then switch in that pane",
)
parser.add_argument(
    "--source-extension", type=pathlib.Path,
    help="Start Pi with this installed integration; require a refusal, then reload the current integration",
)
parser.add_argument(
    "--compacted-source", action="store_true",
    help="Qualify full recorded history separately from the native active-context checkpoint",
)
parser.add_argument("--interrupted-source", action="store_true", help="Include an interrupted historical native tool call before compaction")
parser.add_argument("--chain", default="codex", help="Comma-separated target sequence after Pi, e.g. codex,claude,pi")
parser.add_argument("--local-protocol-fixture", action="store_true", help="Use task-owned native homes and local API capture; operator provider/auth remain untouched")
parser.add_argument("--exercise-turns", action="store_true", help="Send a native model turn through the local fixture before each switch")
parser.add_argument("--handoff-between-switches", action="store_true", help="Handoff the server while Codex is running, then continue the native chain")
parser.add_argument("--restart-between-switches", action="store_true", help="Cold-restart the task-owned server after Codex starts, then continue switching")
parser.add_argument("--fail-native-auth", action="store_true", help="Inject a native account/read failure and require source process/draft preservation")
options = parser.parse_args()
if options.fail_native_auth and (not options.local_protocol_fixture or options.fail_target_launch):
    parser.error("fail-native-auth requires the local fixture and cannot combine with fail-target-launch")
if options.compacted_source and (options.empty_source or options.verify_model_recall):
    parser.error("compacted-source qualification uses its own active-context markers")
if options.empty_source and options.verify_model_recall:
    parser.error("an empty source has no recall markers")
if options.fail_target_launch and options.verify_model_recall:
    parser.error("failure and model-recall modes are mutually exclusive")
settings_path = (
    pathlib.Path(
        os.environ.get("PI_CODING_AGENT_DIR", str(pathlib.Path.home() / ".pi/agent"))
    )
    / "settings.json"
)
settings = json.loads(settings_path.read_text())
source_provider = settings.get("defaultProvider")
source_model = settings.get("defaultModel")
if not source_provider or not source_model:
    raise SystemExit(
        "Pi must have a configured native default provider/model before qualification"
    )
root = options.output.resolve()
root.mkdir(parents=True, exist_ok=True)
base = pathlib.Path(tempfile.mkdtemp(prefix="ohsame-", dir="/var/tmp"))
name = "switch-" + base.name.split("-")[-1]
binary = options.binary.resolve()
source_id = str(uuid.uuid4())
stamp = datetime.datetime.now(datetime.timezone.utc).isoformat().replace("+00:00", "Z")
source = base / "source.jsonl"
extension = (
    pathlib.Path(__file__).resolve().parents[1]
    / "src/integration/assets/pi/herdr-agent-state.ts"
)
usage = {
    "input": 0,
    "output": 0,
    "cacheRead": 0,
    "cacheWrite": 0,
    "totalTokens": 0,
    "cost": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "total": 0},
}
oldest_marker = "OLDEST-" + uuid.uuid4().hex
tool_marker = "TOOL-TAIL-" + uuid.uuid4().hex
records = [
    {
        "type": "session",
        "version": 3,
        "id": source_id,
        "timestamp": stamp,
        "cwd": str(base),
    }
]
parent = None
for i in range(150):
    for role in ["user", "assistant"]:
        eid = uuid.uuid4().hex[:8]
        msg = {
            "role": role,
            "content": [
                {"type": "text", "text": f"RETAIN-{role}-{i:04d}-" + ("x" * 120)}
            ],
            "timestamp": int(time.time() * 1000),
        }
        if i == 0 and role == "user":
            msg["content"][0]["text"] += (
                "\nThe exact oldest retention key is " + oldest_marker
            )
        if role == "assistant":
            msg.update(
                api="openai-responses",
                provider=source_provider,
                model=source_model,
                usage=usage,
                stopReason="stop",
            )
        records.append(
            {
                "type": "message",
                "id": eid,
                "parentId": parent,
                "timestamp": stamp,
                "message": msg,
            }
        )
        parent = eid
call_id = "historical-read"
eid = uuid.uuid4().hex[:8]
records.append(
    {
        "type": "message",
        "id": eid,
        "parentId": parent,
        "timestamp": stamp,
        "message": {
            "role": "assistant",
            "content": [
                {
                    "type": "toolCall",
                    "id": call_id,
                    "name": "read",
                    "arguments": {"path": "HISTORICAL-ARGUMENTS"},
                }
            ],
            "timestamp": int(time.time() * 1000),
            "api": "openai-responses",
            "provider": source_provider,
            "model": source_model,
            "usage": usage,
            "stopReason": "toolUse",
        },
    }
)
parent = eid
eid = uuid.uuid4().hex[:8]
records.append(
    {
        "type": "message",
        "id": eid,
        "parentId": parent,
        "timestamp": stamp,
        "message": {
            "role": "toolResult",
            "toolCallId": call_id,
            "toolName": "read",
            "content": [
                {
                    "type": "text",
                    "text": "FULL-RESULT-"
                    + ("z" * 30000)
                    + "\nThe exact tool-result tail retention key is "
                    + tool_marker,
                }
            ],
            "isError": False,
            "timestamp": int(time.time() * 1000),
        },
    }
)
if options.interrupted_source:
    if not options.compacted_source:
        parser.error("interrupted-source requires compacted-source")
    interrupted = uuid.uuid4().hex[:8]
    records.append({"type":"message", "id":interrupted, "parentId":eid, "timestamp":stamp,
        "message":{"role":"assistant","content":[{"type":"toolCall","id":"historic-interrupted","name":"read","arguments":{"path":"HISTORICAL-INTERRUPTED-ARGS"}}],
                   "timestamp":int(time.time()*1000),"api":"openai-responses","provider":source_provider,"model":source_model,
                   "usage":usage,"stopReason":"error","errorMessage":"historical interruption"}})
    eid = interrupted
if options.compacted_source:
    kept = uuid.uuid4().hex[:8]
    records.append({"type": "message", "id": kept, "parentId": eid, "timestamp": stamp,
                    "message": {"role": "user", "content": "COMPACTED-CURRENT-CONTEXT", "timestamp": int(time.time() * 1000)}})
    diagnostic = uuid.uuid4().hex[:8]
    records.append({"type": "message", "id": diagnostic, "parentId": kept, "timestamp": stamp,
                    "message": {"role": "bashExecution", "command": "fixture-only-no-execution", "output": "EXCLUDED-DIAGNOSTIC",
                                "exitCode": 0, "truncated": False, "cancelled": False, "excludeFromContext": True,
                                "timestamp": int(time.time() * 1000)}})
    records.append({"type": "compaction", "id": uuid.uuid4().hex[:8], "parentId": diagnostic, "timestamp": stamp,
                    "summary": "COMPACTED-SOURCE-SUMMARY", "firstKeptEntryId": kept, "tokensBefore": 12000})
if options.empty_source:
    records = records[:1]
source.write_text("".join(json.dumps(r) + "\n" for r in records))
source_hash = hashlib.sha256(source.read_bytes()).hexdigest()
env = os.environ.copy()
for k in list(env):
    if k.startswith("HERDR_") and k != "HERDR_ENV":
        env.pop(k)
env.update(
    HERDR_CONFIG_PATH=str(base / "config.toml"),
    XDG_CONFIG_HOME=str(base / "config"),
    XDG_STATE_HOME=str(base / "state"),
    HERDR_SESSION=name,
    SHELL="/bin/sh",
)
fixture = None
if options.local_protocol_fixture:
    from native_protocol_fixture import NativeFixture
    fixture=NativeFixture()
    fixture.configure_environment(env,base,binary,extension)
    if options.fail_native_auth:
        stub=base/"auth-failure-api.py"
        stub.write_text('import json,sys\nfor line in sys.stdin:\n r=json.loads(line)\n if "id" not in r:continue\n body={"result":{"userAgent":"native-auth-fault"}} if r["method"]=="initialize" else {"error":{"code":-32603,"message":"workspace routing discovery unauthorized (401)"}}\n print(json.dumps({"id":r["id"],**body}),flush=True)\n')
        native=shutil.which("codex")
        (base/"fixture-bin/codex").write_text('#!/bin/sh\nif [ "$1" = "--no-daemon" ] && [ "$2" = "app-server" ]; then exec python3 '+shlex.quote(str(stub))+'; fi\nexec '+shlex.quote(native)+' --dangerously-bypass-hook-trust "$@"\n')
if options.fail_target_launch:
    shim = base / "bin"
    shim.mkdir()
    command = shim / "codex"
    native_codex = shutil.which("codex")
    if not native_codex:
        raise SystemExit("Codex must be installed before qualification")
    command.write_text(
        '#!/bin/sh\nif [ "$1" = "--version" ] || [ "$1" = "app-server" ] || { [ "$1" = "--no-daemon" ] && [ "$2" = "app-server" ]; }; then exec '
        + shlex.quote(native_codex)
        + ' "$@"; fi\necho task-owned-target-launch-failure >&2\nexit 23\n'
    )
    command.chmod(0o755)
    env["PATH"] = str(shim) + os.pathsep + env.get("PATH", "")
(base / "config.toml").write_text(
    "onboarding=false\n[experimental]\nallow_nested=true\n[update]\nversion_check=false\nmanifest_check=false\n"
)
api_path = None
client_path = None
log = (base / "server.log").open("w")
server_binary = options.handoff_from.resolve() if options.handoff_from else binary
server = subprocess.Popen(
    [str(server_binary), "--session", name, "server"],
    env=env,
    stdin=subprocess.DEVNULL,
    stdout=log,
    stderr=log,
)
client = None
report = {"base": str(base), "source_session": source_id, "source_hash": source_hash}
restore_observer=None


def api(method, params={}, expected_error=None):
    from native_test_client import request
    o=request(api_path,method,params)
    if "error" in o:
        if o["error"]["code"] == expected_error:
            return o["error"]
        raise RuntimeError(f"{method}: {o['error']}")
    if expected_error is not None:
        raise RuntimeError(f"{method} accepted input during the native transition")
    return o["result"]


def varint(n):
    if n < 251:
        return bytes([n])
    if n < 65536:
        return b"\xfb" + struct.pack("<H", n)
    return b"\xfc" + struct.pack("<I", n)


def encode(s):
    b = s.encode()
    return varint(len(b)) + b


def decodeint(d, o):
    n = d[o]
    o += 1
    if n < 251:
        return n, o
    size = {251: 2, 252: 4, 253: 8}[n]
    return int.from_bytes(d[o : o + size], "little"), o + size


def decodestr(d, o):
    n, o = decodeint(d, o)
    return d[o : o + n].decode(), o + n


def exact(n):
    b = b""
    while len(b) < n:
        x = client.recv(n - len(b))
        if not x:
            raise RuntimeError("client endpoint disconnected")
        b += x
    return b


def frame():
    n = struct.unpack("<I", exact(4))[0]
    assert 0 < n < 2097152
    d = exact(n)
    v, o = decodeint(d, 0)
    return v, d[o:]


def send(d):
    client.sendall(struct.pack("<I", len(d)) + d)


def endpoint(method, params):
    send(
        varint(15)
        + encode(boot)
        + encode(json.dumps({"id": method, "method": method, "params": params}))
    )
    response = b""
    while True:
        v, d = frame()
        if v != 18:
            continue
        b, o = decodestr(d, 0)
        rid, o = decodestr(d, o)
        assert b == boot and rid == method
        final = d[o] != 0
        n, o = decodeint(d, o + 1)
        assert len(d) - o == n
        response += d[o:]
        if final:
            return json.loads(response)


try:
    deadline = time.monotonic() + 15
    while client_path is None:
        assert server.poll() is None, "server exited"
        assert time.monotonic() < deadline, "server start timeout"
        sessions = json.loads(
            subprocess.check_output(
                [str(binary), "session", "list", "--json"],
                env=env,
                text=True,
                timeout=5,
            )
        )["sessions"]
        item = next(
            (item for item in sessions if item["name"] == name and item["running"]),
            None,
        )
        if item:
            api_path = pathlib.Path(item["socket_path"])
            sockets = list(pathlib.Path(item["session_dir"]).glob("*client.sock"))
            if sockets:
                client_path = sockets[0]
        if client_path is None:
            time.sleep(0.05)
    created = api("workspace.create", {"cwd": str(base), "focus": True})
    pane = created["root_pane"]["pane_id"]
    terminal = created["root_pane"]["terminal_id"]
    tab = created["tab"]["tab_id"]
    report.update(pane=pane, terminal=terminal, tab=tab)
    def server_version():
        status = subprocess.run(
            [str(binary), "--session", name, "status", "server", "--json"],
            env=env, capture_output=True, text=True, timeout=10,
        )
        return json.loads(status.stdout)

    def live_handoff():
        # Reproduce an upgraded client attached to a stale server: the pane and
        # its running source agent belong to the old server and must survive.
        global terminal, client_path
        expected = subprocess.check_output([str(binary), "--version"], text=True).split()[-1]
        pre_handoff = api("pane.process_info", {"pane_id": pane})["process_info"]
        report["pre_handoff_status"] = server_version()
        handoff = subprocess.run(
            [str(binary), "--session", name, "server", "live-handoff",
             "--import-exe", str(binary), "--expected-version", expected],
            env=env, capture_output=True, text=True, timeout=300,
        )
        report["handoff_output"] = handoff.stdout + handoff.stderr
        assert handoff.returncode == 0, report["handoff_output"]
        post_status = server_version()
        report["post_handoff_status"] = post_status
        assert expected in json.dumps(post_status), post_status
        post_handoff = api("pane.process_info", {"pane_id": pane})["process_info"]
        for key in ["shell_pid", "foreground_process_group_id"]:
            assert post_handoff[key] == pre_handoff[key], (pre_handoff, post_handoff)
        panes = api("pane.list")["panes"]
        assert len(panes) == 1 and panes[0]["pane_id"] == pane, panes
        # Herdr's handoff import allocates fresh terminal identities; the
        # replacement must keep the identity the pane has after the handoff.
        report["pre_handoff_terminal"] = terminal
        terminal = panes[0]["terminal_id"]
        report["terminal"] = terminal
        client_path = next(pathlib.Path(item["session_dir"]).glob("*client.sock"))
        report["handoff_verified"] = True

    def connect_client():
        global client, boot
        client = socket.socket(socket.AF_UNIX)
        client.settimeout(50)
        client.connect(str(client_path))
        hello = {
            "generation": 1,
            "cell_width_px": 8,
            "cell_height_px": 16,
            "surface_size": {"cols": 150, "rows": 45},
            "pixel_mouse": False,
            "direct_graphics": False,
            "endpoint_keybindings": False,
            "mouse_capture": False,
            "snapshot_codecs": ["shell.snapshot.v1"],
            "surface_codecs": ["shell.surface.v1"],
            "input_codecs": ["shell.input.semantic.v1"],
            "blob_codecs": ["shell.blob.v1"],
        }
        send(varint(20) + encode("endpoint.hello.v1") + encode(json.dumps(hello)))
        v, d = frame()
        kind, o = decodestr(d, 0)
        text, o = decodestr(d, o)
        welcome = json.loads(text)
        assert "agent.harness.replace" in welcome["methods"], welcome
        while True:
            v, d = frame()
            if v == 20:
                kind, o = decodestr(d, 0)
                if kind == "shell.snapshot.v1":
                    text, o = decodestr(d, o)
                    boot = json.loads(text)["boot_id"]
                    break
        if "client_shell.surface.set" in welcome["methods"]:
            active=endpoint("client_shell.surface.set",{"active":True})
            assert "error" not in active,active

    if not options.handoff_from:
        connect_client()
    source_extension = extension
    if options.source_extension:
        source_extension = base / "source-extension.ts"
        shutil.copyfile(options.source_extension, source_extension)
    api("integration.harness.set", {"target": "codex", "active": True})
    if not options.direct_source:
        api(
            "agent.profile.create",
            {
                "role": "same-pane-source",
                "harness": "pi",
                "native_cwd": str(base),
                "instructions": "Preserve this synthetic conversation; do not replay historical tool calls.",
            },
        )
    deadline = time.monotonic() + 15
    while True:
        try:
            started = api(
                "agent.start",
                {
                    "name": "same-pane-source",
                    "kind": "pi",
                    "pane_id": pane,
                    "args": [
                        "--no-extensions",
                        "--extension",
                        str(source_extension),
                        "--session",
                        str(source),
                        "--no-mcp",
                    ],
                    "timeout_ms": 15000,
                },
            )
            break
        except RuntimeError:
            assert time.monotonic() < deadline
            time.sleep(0.1)
    api("agent.ready.wait", {"target": pane, "timeout_ms": 30000})
    if options.handoff_from:
        live_handoff()
        connect_client()
    prepared = api("agent.harness.prepare", {"target": pane})
    instance = prepared["instance_id"]
    report["instance"] = instance
    if options.source_extension:
        # The running source integration never registered its native context:
        # the switch must be refused before anything changes.
        stale = endpoint(
            "agent.harness.replace", {"instance_id": instance, "harness": "codex"}
        )
        report["stale_integration_refusal"] = stale
        assert stale.get("error", {}).get("code") == "native_context_unavailable", stale
        assert api("agent.get", {"target": pane})["agent"]["agent"] == "pi"
        assert len(api("pane.list")["panes"]) == 1
        # Remediation: install the current integration, reload it in place.
        shutil.copyfile(extension, source_extension)
        api("pane.send_text", {"pane_id": pane, "text": "/reload"})
        api("pane.send_keys", {"pane_id": pane, "keys": ["Enter"]})
        api("agent.ready.wait", {"target": pane, "timeout_ms": 30000})
        # Registration retries at most 3 x 1.5s after session_start.
        time.sleep(6)
        report["reloaded_integration"] = True
    # Allow registration retries to settle without changing the native source.
    time.sleep(0.3)
    before = api("pane.process_info", {"pane_id": pane})["process_info"]
    report["before_process"] = before
    pre_switch_source_hash=hashlib.sha256(source.read_bytes()).hexdigest()
    api("pane.send_text", {"pane_id": pane, "text": "TASK-OWNED-UNSENT-DRAFT"})
    refused = endpoint(
        "agent.harness.replace", {"instance_id": instance, "harness": "codex"}
    )
    if options.fail_native_auth:
        assert refused.get('error',{}).get('code')=='native_target_unavailable',refused
        assert api('pane.process_info',{'pane_id':pane})['process_info']['foreground_process_group_id']==before['foreground_process_group_id']
        screen=api('pane.read',{'pane_id':pane,'source':'detection','lines':80})['read']['text']
        assert 'TASK-OWNED-UNSENT-DRAFT' in screen,screen
        assert hashlib.sha256(source.read_bytes()).hexdigest()==pre_switch_source_hash
        report.update(success=True,auth_rejection_preserved_source=True,preflight_refusal=refused)
        raise SystemExit(0)
    assert refused.get("error", {}).get("code") == "source_has_draft", refused
    assert (
        api("pane.process_info", {"pane_id": pane})["process_info"][
            "foreground_process_group_id"
        ]
        == before["foreground_process_group_id"]
    )
    assert len(api("pane.list")["panes"]) == 1
    api("pane.send_keys", {"pane_id": pane, "keys": ["Ctrl+c"]})
    report["draft_rejection_verified"] = True
    observations = []
    stop_observation = threading.Event()

    def observe_target():
        pause_checked = False
        while not stop_observation.wait(0.5):
            if not pause_checked:
                pause_checked = True
                try:
                    api(
                        "pane.send_text",
                        {"pane_id": pane, "text": "TASK_OWNED_MUST_NOT_REACH_SHELL"},
                        expected_error="harness_switch_busy",
                    )
                    report["transition_input_pause_verified"] = True
                except Exception as error:
                    report["transition_input_pause_failure"] = str(error)
            try:
                observations.append(api("agent.get", {"target": pane})["agent"])
            except Exception:
                pass

    observer = threading.Thread(target=observe_target, daemon=True)
    observer.start()
    result = endpoint(
        "agent.harness.replace", {"instance_id": instance, "harness": "codex"}
    )
    stop_observation.set()
    observer.join(timeout=4)
    report["observations"] = observations[-25:]
    report["replacement"] = result
    assert report.get("transition_input_pause_verified"), report.get(
        "transition_input_pause_failure", "native input pause was not observed"
    )
    if options.fail_target_launch:
        assert result.get("error", {}).get("code") == "native_switch_timeout", result
        recovery = json.loads((api_path.parent / "agents.json").read_text())[
            "native_harness_recovery"
        ][instance]
        deadline = time.monotonic() + 35
        restored = None
        while time.monotonic() < deadline:
            try:
                candidate = api("agent.get", {"target": pane})["agent"]
                if (
                    candidate.get("agent") == "pi"
                    and candidate.get("agent_session", {}).get("value")
                    == recovery["source_archive"]
                ):
                    restored = candidate
                    break
            except RuntimeError:
                pass
            time.sleep(0.1)
        assert restored, (
            "the exact source branch did not resume after the target failed"
        )
        assert (
            restored["terminal_id"] == terminal
            and restored["pane_id"] == pane
            and restored["tab_id"] == tab
        )
        assert len(api("pane.list")["panes"]) == 1
        assert (
            api("pane.process_info", {"pane_id": pane})["process_info"]["shell_pid"]
            == before["shell_pid"]
        )
        assert (
            json.loads(
                pathlib.Path(recovery["source_archive"]).read_text().splitlines()[0]
            )["id"]
            == source_id
        )
        report.update(success=True, rollback_verified=True, restored_agent=restored)
        print(
            json.dumps(
                {
                    "success": True,
                    "rollback_verified": True,
                    "base": str(base),
                    "pane": pane,
                },
                indent=2,
            )
        )
        raise SystemExit(0)
    if "error" in result:
        raise RuntimeError(str(result["error"]))
    assert result["result"]["pane"]["pane_id"] == pane
    assert result["result"]["messages"] >= (0 if options.empty_source else 302)
    panes = api("pane.list")["panes"]
    tabs = api("tab.list", {"workspace_id": created["workspace"]["workspace_id"]})[
        "tabs"
    ]
    assert len(panes) == 1 and len(tabs) == 1, (len(panes), len(tabs))
    after = api("pane.process_info", {"pane_id": pane})["process_info"]
    assert after["shell_pid"] == before["shell_pid"]
    report["after_process"] = after
    agent = api("agent.get", {"target": pane})["agent"]
    assert (
        agent["terminal_id"] == terminal
        and agent["tab_id"] == tab
        and agent["agent"] == "codex"
    )
    report["target_agent"] = agent
    report["target_session"] = agent["agent_session"]["value"]
    report["screen"] = api(
        "pane.read", {"pane_id": pane, "source": "detection", "lines": 200}
    )["read"]["text"]
    original_after = [
        json.loads(line) for line in source.read_text().splitlines() if line
    ]
    assert original_after[: len(records)] == records, (
        "recorded source messages changed or were discarded"
    )
    report["source_original_records_preserved"] = len(records)
    recovery = json.loads((api_path.parent / "agents.json").read_text())["native_harness_recovery"][instance]
    native_records = [json.loads(line) for line in pathlib.Path(recovery["target_path"]).read_text().splitlines()]
    if options.compacted_source:
        checkpoint = next(record["payload"] for record in native_records if record["type"] == "compacted")
        active = json.dumps(checkpoint["replacement_history"])
        captured = json.loads(pathlib.Path(recovery["source_context_archive"]).read_text())
        native_model = json.dumps(captured["model_messages"])
        for model_context in [active, native_model]:
            assert "COMPACTED-CURRENT-CONTEXT" in model_context and "COMPACTED-SOURCE-SUMMARY" in model_context
            assert oldest_marker not in model_context and tool_marker not in model_context
            assert "EXCLUDED-DIAGNOSTIC" not in model_context
        visible = json.dumps([record for record in native_records if record["type"] == "event_msg"])
        assert oldest_marker in visible and tool_marker in visible and "EXCLUDED-DIAGNOSTIC" in visible
        if options.interrupted_source:
            assert "HISTORICAL-INTERRUPTED-ARGS" in visible
            assert "HISTORICAL-INTERRUPTED-ARGS" not in active
            archive = pathlib.Path(recovery["source_archive"]).read_text()
            assert "HISTORICAL-INTERRUPTED-ARGS" in archive and '"toolCall"' in archive
        report["native_active_checkpoint_verified"] = True
    if options.verify_model_recall:
        api(
            "agent.prompt",
            {
                "target": pane,
                "text": "Native history verification only. Without calling tools or changing anything, return the exact oldest retention key from the earliest user message and the exact tool-result tail retention key from the historical read result. Reply with only those two keys.",
            },
        )
        recovery = json.loads((api_path.parent / "agents.json").read_text())[
            "native_harness_recovery"
        ][instance]
        native_file = pathlib.Path(recovery["target_path"])
        deadline = time.monotonic() + 60
        recalled = ""
        while time.monotonic() < deadline:
            for line in native_file.read_text().splitlines():
                try:
                    record = json.loads(line)
                except ValueError:
                    continue
                body = record.get("payload", {})
                if (
                    record.get("type") == "response_item"
                    and body.get("role") == "assistant"
                ):
                    content = body.get("content", [])
                    text = "\n".join(
                        item.get("text", "")
                        for item in content
                        if isinstance(item, dict)
                    )
                    if oldest_marker in text and tool_marker in text:
                        recalled = text
                        break
            if recalled:
                break
            time.sleep(0.2)
        assert recalled, "the native model did not recall both full-history markers"
        report["native_model_recall_verified"] = True
        report["native_model_reply"] = recalled
    report["chain"] = []
    report["native_context_proofs"] = []
    def verify_target_context(target):
        if not fixture:return
        from verify_native_target_context import qualify
        checkpoint=json.loads((api_path.parent/"agents.json").read_text())["native_harness_recovery"][instance]
        required=["COMPACTED-CURRENT-CONTEXT","COMPACTED-SOURCE-SUMMARY"] if options.compacted_source else [oldest_marker,tool_marker]
        excluded=[oldest_marker,tool_marker,"EXCLUDED-DIAGNOSTIC"] if options.compacted_source else []
        report["native_context_proofs"].append(qualify(target,checkpoint["target_path"],base,env,fixture,required,excluded))
        print("native context verified: "+target,file=sys.stderr,flush=True)
        if options.exercise_turns and target != "pi":
            start=len(fixture.requests)
            api("agent.prompt",{"target":pane,"text":"Without tools, reply CAPTURED."})
            deadline=time.monotonic()+25
            while time.monotonic()<deadline:
                screen=api("pane.read",{"pane_id":pane,"source":"detection","lines":20})["read"]["text"]
                output_seen=("• CAPTURED" in screen or "⏺ CAPTURED" in screen)
                if output_seen and len(fixture.requests)>start:break
                time.sleep(.1)
            assert len(fixture.requests)>start and output_seen,screen
            api("agent.ready.wait",{"target":pane,"timeout_ms":15000})
            report.setdefault("native_tui_turns",[]).append(target)
    verify_target_context("codex")
    if options.handoff_between_switches:
        client.close();client=None
        live_handoff();connect_client()
    if options.restart_between_switches:
        old_session=api("agent.get",{"target":pane})["agent"]["agent_session"]
        client.close();client=None
        api("server.stop");server.wait(timeout=15)
        server=subprocess.Popen([str(binary),"--session",name,"server"],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log)
        deadline=time.monotonic()+15
        while time.monotonic()<deadline:
            if client_path.exists():
                try:
                    connect_client();break
                except (ConnectionRefusedError,FileNotFoundError):pass
            time.sleep(.1)
        assert client is not None,"restart client unavailable"
        focused=endpoint("pane.focus",{"pane_id":pane})
        assert "error" not in focused,focused
        restore_observer=subprocess.Popen([str(binary),"--session",name,"terminal","session","observe",pane,"--cols","150","--rows","45"],env=env,stdin=subprocess.DEVNULL,stdout=(base/"restore-observer.log").open("w"),stderr=subprocess.STDOUT)
        deadline=time.monotonic()+20
        while time.monotonic()<deadline:
            try:
                api("pane.process_info",{"pane_id":pane});break
            except RuntimeError:time.sleep(.1)
        api("agent.ready.wait",{"target":pane,"timeout_ms":30000})
        restored=api("agent.get",{"target":pane})["agent"]
        assert restored["agent_session"]==old_session,restored
        terminal=restored["terminal_id"];report["terminal"]=terminal
        report["cold_restart_verified"]=True
    for target in options.chain.split(","):
        if target == "codex" and not report["chain"]:
            report["chain"].append({"target":"codex","result":report["replacement"]})
            continue
        assert target in {"pi","codex","claude"}, target
        api("integration.harness.set", {"target":target,"active":True})
        identity = api("pane.process_info", {"pane_id":pane})["process_info"]
        api("pane.send_text", {"pane_id":pane,"text":"TASK-OWNED-CHAIN-DRAFT"})
        refusal=endpoint("agent.harness.replace", {"instance_id":instance,"harness":target})
        assert refusal.get("error",{}).get("code")=="source_has_draft",refusal
        assert api("pane.process_info", {"pane_id":pane})["process_info"]["foreground_process_group_id"]==identity["foreground_process_group_id"]
        # Wait for the real frontend to restore the preserved editor buffer.
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            draft=api("pane.read", {"pane_id":pane,"source":"detection","lines":20})["read"]["text"]
            if "TASK-OWNED-CHAIN-DRAFT" in draft:break
            time.sleep(.05)
        assert "TASK-OWNED-CHAIN-DRAFT" in draft,draft
        api("pane.send_keys", {"pane_id":pane,"keys":["Ctrl+c"]})
        switch = endpoint("agent.harness.replace", {"instance_id":instance,"harness":target})
        report["chain"].append({"target":target,"result":switch})
        assert "error" not in switch, switch
        switched = switch["result"]
        assert switched["pane"]["pane_id"] == pane and switched["pane"]["terminal_id"] == terminal, switched
        assert api("pane.process_info", {"pane_id":pane})["process_info"]["shell_pid"] == identity["shell_pid"]
        assert len(api("pane.list")["panes"]) == 1
        recovery=json.loads((api_path.parent/"agents.json").read_text())["native_harness_recovery"][instance]
        loaded=pathlib.Path(recovery["target_path"]).read_text()
        assert oldest_marker in loaded and tool_marker in loaded, "chain lost historical markers"
        if options.interrupted_source:
            assert "HISTORICAL-INTERRUPTED-ARGS" in loaded, "chain lost interrupted historical tool arguments"
        verify_target_context(target)
    report["success"] = True
except Exception as error:
    report.update(success=False, error=str(error))
    if api_path and api_path.exists():
        try:
            report["screen"] = api(
                "pane.read",
                {"pane_id": report.get("pane"), "source": "detection", "lines": 120},
            )["read"]["text"]
        except Exception:
            pass
finally:
    if restore_observer is not None and restore_observer.poll() is None:
        restore_observer.terminate();restore_observer.wait(timeout=5)
    if client:
        client.close()
    try:
        api("server.stop")
        server.wait(timeout=10)
    except Exception:
        if options.handoff_from:
            subprocess.run([str(binary), "--session", name, "server", "stop"],
                           env=env, capture_output=True, timeout=15)
        if server.poll() is None:
            server.terminate()
            server.wait(timeout=5)
    log.close()
    if fixture:
        (base/"captured-native-requests.json").write_text(json.dumps(fixture.requests,indent=2))
        fixture.close()
    (root / "same-pane-native-result.json").write_text(
        json.dumps(report, indent=2) + "\n"
    )
    (base / "result.json").write_text(json.dumps(report, indent=2) + "\n")
print(
    json.dumps(
        {
            k: report.get(k)
            for k in ["success", "error", "base", "pane", "terminal", "target_session"]
        },
        indent=2,
    )
)
raise SystemExit(0 if report.get("success") else 1)
