"""Qualify imported Codex context using a local fixed-response HTTP fixture.

Consumes a rollout produced by the Rust importer. No credentials, remote model,
provider configuration changes, or historical tool execution are involved.
"""
import argparse
import http.server
import json
import os
import pathlib
import queue
import subprocess
import threading
import time


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--home", type=pathlib.Path, required=True)
    parser.add_argument("--session", required=True)
    parser.add_argument("--cwd", type=pathlib.Path, required=True)
    args = parser.parse_args()
    captured = []

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args):
            pass

        def do_POST(self):
            captured.append(json.loads(self.rfile.read(int(self.headers["Content-Length"]))))
            item = {"id": "msg_fixture", "type": "message", "role": "assistant", "status": "completed",
                    "content": [{"type": "output_text", "text": "CAPTURED", "annotations": []}]}
            events = [
                {"type": "response.created", "response": {"id": "resp_fixture", "status": "in_progress", "output": []}},
                {"type": "response.output_item.added", "output_index": 0, "item": {"id": "msg_fixture", "type": "message", "role": "assistant", "content": []}},
                {"type": "response.output_text.delta", "output_index": 0, "content_index": 0, "item_id": "msg_fixture", "delta": "CAPTURED"},
                {"type": "response.output_item.done", "output_index": 0, "item": item},
                {"type": "response.completed", "response": {"id": "resp_fixture", "status": "completed", "output": [item], "usage": {"input_tokens": 1, "output_tokens": 1, "total_tokens": 2}}},
            ]
            body = "".join("event: " + event["type"] + "\ndata: " + json.dumps(event) + "\n\n" for event in events).encode()
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    config = args.home / "config.toml"
    assert not config.exists(), "only a fresh task-owned native home is allowed"
    config.write_text(
        'model_provider="capture"\nmodel="fixture-model"\n[model_providers.capture]\n'
        'name="Local fixed-response request capture"\n'
        f'base_url="http://127.0.0.1:{server.server_port}/v1"\n'
        'wire_api="responses"\nrequires_openai_auth=false\nsupports_websockets=false\n'
    )
    env = os.environ.copy()
    env.update(CODEX_HOME=str(args.home), NO_PROXY="127.0.0.1,localhost")
    for key in ["OPENAI_API_KEY", "CODEX_API_KEY"]:
        env.pop(key, None)
    try:
        result = subprocess.run(
            ["codex", "exec", "--skip-git-repo-check", "--sandbox", "read-only", "resume", args.session,
             "Do not call any tools. Reply CAPTURED."],
            env=env, cwd=args.cwd, stdin=subprocess.DEVNULL, capture_output=True, text=True, timeout=30,
        )
        assert result.returncode == 0, result.stderr
        assert captured, "no native request was captured"
        (args.home.parent / "captured-native-request.json").write_text(json.dumps(captured, indent=2))
        for request in captured:
            active = json.dumps(request.get("input", []))
            assert "summary only" in active, "native active-context checkpoint was lost"
            assert "user-0-" not in active, "precompaction history was resurrected in model context"
            assert "kept-output-" + "z" * 30000 in active, "retained active tool output was truncated"
            assert "kept-arguments" in active, "active tool arguments were lost"
            assert active.count("data:image/") >= 8, "active user/tool PNG, JPEG, WebP or GIF images were lost"
        response = read_history(env, args.session)
        visible = json.dumps(response)
        for marker in ["user-0-", "assistant-299-", "kept-arguments", "kept-output-" + "z" * 30000]:
            assert marker in visible, "native visible history lost " + marker[:40]
        assert len(response["result"]["thread"]["turns"]) >= 300
        print(json.dumps({"success": True, "native_requests": len(captured), "complete_history_retained": True,
                          "active_checkpoint_honored": True}))
    finally:
        server.shutdown()
        server.server_close()


def read_history(env, session):
    process = subprocess.Popen(["codex", "app-server"], env=env, stdin=subprocess.PIPE,
                               stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
    responses = queue.Queue()

    def read():
        for line in process.stdout:
            responses.put(json.loads(line))

    reader = threading.Thread(target=read, daemon=True)
    reader.start()

    def request(identifier, method, params):
        process.stdin.write(json.dumps({"id": identifier, "method": method, "params": params}) + "\n")
        process.stdin.flush()
        deadline = time.monotonic() + 20
        while True:
            response = responses.get(timeout=max(0, deadline - time.monotonic()))
            if response.get("id") == identifier:
                assert "error" not in response, response
                return response

    try:
        request(1, "initialize", {"clientInfo": {"name": "ohmyherdr-checkpoint-fixture", "version": "1"}})
        process.stdin.write('{"method":"initialized"}\n')
        process.stdin.flush()
        return request(2, "thread/read", {"threadId": session, "includeTurns": True})
    finally:
        process.kill()
        process.wait()
        process.stdin.close()
        reader.join(timeout=2)
        process.stdout.close()


if __name__ == "__main__":
    main()
