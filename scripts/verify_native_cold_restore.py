"""Verify exact native cold restoration after processes exit in retained panes.
Uses a task-owned server, native homes, and the existing local protocol fixture.
"""
import argparse
import fcntl
import json
import os
import pathlib
import pty
import signal
import struct
import subprocess
import tempfile
import termios
import threading
import time
import uuid

from native_protocol_fixture import NativeFixture
from native_test_client import call, request


def wait_until(check, timeout, description):
    deadline = time.monotonic() + timeout
    while time.monotonic() < deadline:
        value = check()
        if value:
            return value
        time.sleep(.1)
    raise AssertionError(description)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, required=True)
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--pi-panes', type=int, default=3, choices=range(1, 31))
    args = parser.parse_args()
    binary = args.binary.resolve()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    base = pathlib.Path(tempfile.mkdtemp(prefix='ohresume-', dir='/tmp')).resolve()
    env = {k: v for k, v in os.environ.items() if not k.startswith('HERDR_')}
    name = 'resume-' + uuid.uuid4().hex[:8]
    env.update(HERDR_ENV='1', HERDR_SESSION=name, HERDR_CONFIG_PATH=str(base/'config.toml'),
               XDG_CONFIG_HOME=str(base/'config'), XDG_STATE_HOME=str(base/'state'), SHELL='/bin/sh')
    fixture = NativeFixture()
    extension = pathlib.Path(__file__).resolve().parents[1]/'src/integration/assets/pi/herdr-agent-state.ts'
    fixture.configure_environment(env, base, binary, extension)
    (base/'config.toml').write_text('onboarding=false\n[terminal]\ndefault_shell="/bin/sh"\n[experimental]\nallow_nested=true\n[update]\nversion_check=false\nmanifest_check=false\n')
    endpoint = None
    observer = None
    observer_master = None
    observer_reader = None
    server = None
    log = (base/'server.log').open('w')
    report = {'base': str(base), 'sessions': [], 'pi_panes': args.pi_panes}

    def api(method, params=None):
        return call(endpoint, method, params)

    def start_server():
        nonlocal server, endpoint
        server = subprocess.Popen([str(binary), '--session', name, 'server'], env=env,
                                  stdin=subprocess.DEVNULL, stdout=log, stderr=log)
        def locate():
            assert server.poll() is None, 'task-owned server exited'
            sessions = json.loads(subprocess.check_output([str(binary), 'session', 'list', '--json'],
                                 env=env, text=True, timeout=5))['sessions']
            item = next((s for s in sessions if s['name'] == name and s['running']), None)
            return pathlib.Path(item['socket_path']) if item else None
        endpoint = wait_until(locate, 20, 'task-owned server did not start')

    def agent_at(pane):
        return next((p for p in api('agent.list')['agents'] if p['pane_id'] == pane), None)

    def start_agent(pane, kind, native_args, index):
        def attempt():
            result = request(endpoint, 'agent.start', {'name': f'resume-{index}', 'kind': kind,
                             'pane_id': pane, 'args': native_args, 'timeout_ms': 60000})
            if 'error' in result:
                assert result['error']['code'] == 'agent_pane_busy', result
                return False
            return True
        wait_until(attempt, 20, 'task-owned shell did not become available')
        api('agent.ready.wait', {'target': pane, 'timeout_ms': 65000})

    try:
        start_server()
        for kind in ['pi', 'claude', 'codex']:
            api('integration.harness.set', {'target': kind, 'active': True})
        for index, kind in enumerate(['pi'] * args.pi_panes + ['claude', 'codex']):
            created = api('workspace.create', {'cwd': str(base), 'focus': False})
            pane = created['root_pane']['pane_id']
            marker = 'EXACT-COLD-RESTORE-' + uuid.uuid4().hex
            native_args = []
            if kind == 'pi':
                source = base/f'pi-{index}.jsonl'
                native_id = str(uuid.uuid4())
                stamp = '2026-01-01T00:00:00.000Z'
                records = [
                    {'type': 'session', 'version': 3, 'id': native_id, 'timestamp': stamp, 'cwd': str(base)},
                    {'type': 'model_change', 'id': 'model001', 'parentId': None, 'timestamp': stamp,
                     'provider': 'openai', 'modelId': 'gpt-4o'},
                    {'type': 'message', 'id': 'user0001', 'parentId': 'model001', 'timestamp': stamp,
                     'message': {'role': 'user', 'content': marker, 'timestamp': 1}},
                    {'type': 'message', 'id': 'asst0001', 'parentId': 'user0001', 'timestamp': stamp,
                     'message': {'role': 'assistant', 'content': [{'type': 'text', 'text': marker}],
                                 'api': 'openai-completions', 'provider': 'openai', 'model': 'gpt-4o',
                                 'usage': {'input': 1, 'output': 1, 'cacheRead': 0, 'cacheWrite': 0,
                                           'totalTokens': 2, 'cost': {'input': 0, 'output': 0, 'cacheRead': 0, 'cacheWrite': 0, 'total': 0}},
                                 'stopReason': 'stop', 'timestamp': 2}},
                ]
                source.write_text(''.join(json.dumps(r)+'\n' for r in records))
                native_args = ['--session', str(source)]
            start_agent(pane, kind, native_args, index)
            if kind != 'pi':
                before = len(fixture.requests)
                api('agent.prompt', {'target': pane, 'text': 'Without tools, reply CAPTURED. Retain '+marker})
                wait_until(lambda: len(fixture.requests) > before and 'CAPTURED' in
                           api('pane.read', {'pane_id': pane, 'source': 'detection', 'lines': 80})['read']['text'],
                           40, 'native fixture reply missing')
                api('agent.ready.wait', {'target': pane, 'timeout_ms': 30000})
            expected = agent_at(pane)['agent_session']
            report['sessions'].append({'pane': pane, 'kind': kind, 'expected': expected, 'marker': marker})
            print('Native session ready:', pane, kind, flush=True)
        # Signal only each task-owned native foreground group; the original shell stays.
        for entry in report['sessions']:
            info = api('pane.process_info', {'pane_id': entry['pane']})['process_info']
            pgid = info['foreground_process_group_id']
            assert pgid and pgid != info['shell_pid'], info
            os.killpg(pgid, signal.SIGTERM)
            wait_until(lambda: not agent_at(entry['pane']), 20, 'native process did not release its pane')
        api('server.stop')
        server.wait(timeout=15)
        snapshot_path = endpoint.parent/'session.json'
        saved = json.loads(snapshot_path.read_text())
        saved_refs = [p['agent_session'] for w in saved['workspaces'] for t in w['tabs']
                      for p in t['panes'].values() if p.get('agent_session')]
        report['saved_refs'] = saved_refs
        assert len(saved_refs) == len(report['sessions']), 'shutdown erased native resume references from retained panes'
        assert sorted(json.dumps(x, sort_keys=True) for x in saved_refs) == sorted(
            json.dumps(x['expected'], sort_keys=True) for x in report['sessions'])
        start_server()
        # Attach a real TUI in its own PTY so cold restore gets the same global
        # geometry event as a user reopening the app, including hidden panes.
        observer_master, slave = pty.openpty()
        fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 45, 150, 0, 0))
        observer = subprocess.Popen([str(binary), '--session', name], env={**env, 'TERM': 'xterm-256color'},
                                    stdin=slave, stdout=slave, stderr=slave, start_new_session=True)
        os.close(slave)
        def drain_observer():
            with (base/'observer.log').open('wb') as captured:
                while True:
                    try:
                        data = os.read(observer_master, 65536)
                    except OSError:
                        return
                    if not data:
                        return
                    captured.write(data)
        observer_reader = threading.Thread(target=drain_observer, daemon=True)
        observer_reader.start()
        for entry in report['sessions']:
            def restored():
                agent = agent_at(entry['pane'])
                if not agent or agent.get('agent_session') != entry['expected'] or agent.get('launch_pending'):
                    return False
                process = api('pane.process_info', {'pane_id': entry['pane']})['process_info']
                if not process.get('foreground_process_group_id') or process['foreground_process_group_id'] == process['shell_pid']:
                    return False
                screen = api('pane.read', {'pane_id': entry['pane'], 'source': 'recent_unwrapped', 'lines': 120})['read']['text']
                return entry['marker'] in screen
            wait_until(restored, 90, 'exact native session was not automatically resumed: '+entry['pane'])
            entry['restored'] = True
            if entry['kind'] != 'pi':
                before = len(fixture.requests)
                api('agent.ready.wait', {'target': entry['pane'], 'timeout_ms': 30000})
                api('agent.prompt', {'target': entry['pane'], 'text': 'Without tools, reply CAPTURED again.'})
                wait_until(lambda: any(entry['marker'] in json.dumps(r['payload']) for r in fixture.requests[before:]),
                           40, 'resumed native model context lost the original conversation')
                entry['model_context_verified'] = True
        report['success'] = True
    except Exception as error:
        report.update(success=False, error=str(error))
    finally:
        if observer and observer.poll() is None:
            observer.terminate()
            observer.wait(timeout=5)
        if observer_master is not None:
            os.close(observer_master)
        if observer_reader:
            observer_reader.join(timeout=2)
        if server and server.poll() is None:
            try:
                api('server.stop')
                server.wait(timeout=15)
            except Exception:
                server.kill()
                server.wait(timeout=5)
        fixture.close()
        log.close()
        (output/'result.json').write_text(json.dumps(report, indent=2)+'\n')
        print(json.dumps({k: report.get(k) for k in ['success', 'error', 'base', 'pi_panes']}, indent=2))
    return 0 if report.get('success') else 1


if __name__ == '__main__':
    raise SystemExit(main())
