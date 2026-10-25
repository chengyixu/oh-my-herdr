#!/usr/bin/env python3
"""Managed native editor/context bridge. No model/provider/auth configuration.

The server is bound to one foreground native process and one exact session.
A normal external-editor invocation delegates the operator's original editor.
An armed snapshot reads the native buffer without modifying it and holds its
acknowledgement until close/release. All PTY controls go through the fenced app.
"""
import argparse
import hashlib
import hmac
import json
import os
import pathlib
import shlex
import socket
import stat
import subprocess
import threading
import time
import select
import selectors

MAX_BYTES = 256 * 1024 * 1024


def read_private(path):
    path = pathlib.Path(path)
    info = path.lstat()
    if not stat.S_ISREG(info.st_mode) or info.st_uid != os.getuid() or info.st_mode & 0o077:
        raise RuntimeError('native context lease is not private')
    return json.loads(path.read_text())


def exchange(endpoint, request, timeout=15):
    with socket.socket(socket.AF_UNIX) as client:
        client.settimeout(timeout)
        client.connect(str(endpoint))
        client.sendall((json.dumps(request)+'\n').encode())
        response = bytearray()
        while b'\n' not in response:
            chunk = client.recv(65536)
            if not chunk:
                raise RuntimeError('native context connection ended')
            response.extend(chunk)
            if len(response) > MAX_BYTES:
                raise RuntimeError('native context exceeded its bound')
        return json.loads(response.split(b'\n')[0])


def app_control(config, action):
    context = config['context']
    response = exchange(config['api_endpoint'], {'id':'native-source-control', 'method':'agent.context.control',
                                                'params':{'context':context, 'action':action}})
    if 'error' in response:
        raise RuntimeError(response['error']['code']+': '+response['error']['message'])
    return response['result']


def editor(lease, buffer):
    config = read_private(lease)
    try:
        response = exchange(config['endpoint'], {'token':config['token'], 'action':'editor',
                            'parent_pid':os.getppid(), 'editor_pid':os.getpid(), 'buffer_path':str(pathlib.Path(buffer).resolve())}, timeout=45)
    except (OSError, RuntimeError):
        response = {'delegate':True}
    if response.get('delegate'):
        original = os.environ.get('OHMYHERDR_NATIVE_EDITOR_FALLBACK', '')
        command = shlex.split(original)
        if not command:
            raise RuntimeError('neither VISUAL nor EDITOR was configured')
        return subprocess.call(command+[buffer])
    if response.get('ok'):
        return 0
    raise RuntimeError('native editor bridge refused the operation; draft file unchanged')


def wait_editor_exit(pid, timeout=5):
    if not isinstance(pid,int) or pid<=0:return False
    try:os.kill(pid,0)
    except ProcessLookupError:return True
    if hasattr(select,'kqueue'):
        try:
            events=select.kqueue()
            try:
                events.control([select.kevent(pid,filter=select.KQ_FILTER_PROC,flags=select.KQ_EV_ADD|select.KQ_EV_ONESHOT,fflags=select.KQ_NOTE_EXIT)],0,0)
                return bool(events.control(None,1,timeout))
            finally:events.close()
        except ProcessLookupError:return True
    if hasattr(os,'pidfd_open'):
        try:
            descriptor=os.pidfd_open(pid)
            try:return bool(select.select([descriptor],[],[],timeout)[0])
            finally:os.close(descriptor)
        except ProcessLookupError:return True
    if timeout is None:
        raise RuntimeError('native process exit events are unavailable')
    deadline=time.monotonic()+timeout
    while time.monotonic()<deadline:
        try:os.kill(pid,0)
        except ProcessLookupError:return True
        threading.Event().wait(.02)
    return False


def owns_editor_process(source_pid, parent_pid, editor_pid):
    if parent_pid==source_pid:return True
    if not isinstance(editor_pid,int) or not isinstance(parent_pid,int):return False
    try:
        if os.getpgid(editor_pid)!=os.getpgid(source_pid):return False
        current=parent_pid
        for _ in range(4):
            if current==source_pid:return True
            result=subprocess.run(['ps','-p',str(current),'-o','ppid='],capture_output=True,text=True,timeout=1)
            current=int(result.stdout.strip())
            if current<=1:return False
    except (OSError,ValueError,subprocess.TimeoutExpired):return False
    return False


class ContextServer:
    def __init__(self, lease):
        self.lease = lease
        self.condition = threading.Condition()
        self.armed = False
        self.editor_ready = False
        self.editor_error = None
        self.release = False
        self.editor_done = threading.Event()
        self.editor_pid = None
        self.snapshot_id = None
        self.clients = 0
        self.slots = threading.BoundedSemaphore(4)
        self.owner_pid = None
        self.stopped = threading.Event()
        self.wake = None

    def monitor_source(self, config):
        context = config.get('context')
        if not context:
            return
        with self.condition:
            pid = context['process_id']
            if self.owner_pid is not None:
                if self.owner_pid != pid:
                    raise RuntimeError('native_context_not_owned: source process changed')
                return
            self.owner_pid = pid
            threading.Thread(target=self.observe_source_exit,args=(pid,),daemon=True).start()
            if self.wake:
                self.wake.sendall(b'a')

    def observe_source_exit(self, pid):
        try:
            wait_editor_exit(pid, timeout=None)
        finally:
            self.stopped.set()
            try:
                if self.wake:self.wake.sendall(b'x')
            except OSError:pass

    def configuration(self):
        return read_private(self.lease)

    def snapshot(self, config):
        with self.condition:
            if self.armed:
                raise RuntimeError('source_busy: native context export already active')
            inspection = app_control(config, 'inspect')
            if not inspection['queue_unchanged']:
                raise RuntimeError('source_has_unacknowledged_input: native queue/attachments are not proven empty')
            self.armed, self.release, self.editor_ready, self.editor_error = True, False, False, None
            self.editor_done.clear()
        try:
            app_control(config, 'open_editor')
            with self.condition:
                if not self.condition.wait_for(lambda:self.editor_ready or self.editor_error, timeout=10):
                    raise RuntimeError('native_context_unavailable: native editor did not acknowledge')
                if self.editor_error:
                    raise RuntimeError(self.editor_error)
            value = self.capture(config)
            digest = hashlib.sha256(json.dumps(value, sort_keys=True, separators=(',',':')).encode()).hexdigest()
            with self.condition:
                self.snapshot_id = digest
            return {'ok':True,'snapshot_id':digest,'snapshot':value}
        except Exception:
            self.release_editor()
            raise

    def capture(self, config):
        context = config['context']
        result = subprocess.run([config['binary'],'__native-context','snapshot',context['agent'],
                                 context['session_path'],context['session_id']],
                                stdin=subprocess.DEVNULL,capture_output=True,timeout=10)
        if result.returncode or len(result.stdout)>MAX_BYTES:
            raise RuntimeError('native_context_unavailable: native source history could not be exported')
        return json.loads(result.stdout)

    def release_editor(self):
        with self.condition:
            if self.armed and not self.editor_ready and not self.editor_error:
                self.editor_error = 'source_changed: native context preparation was cancelled'
            self.release, self.armed = True, False
            self.condition.notify_all()

    def handle_editor(self, config, command):
        with self.condition:
            if not self.armed:
                return {'delegate':True}
            if not owns_editor_process(config['context']['process_id'],command.get('parent_pid'),command.get('editor_pid')):
                raise RuntimeError('native_context_not_owned: external editor parent changed')
            self.editor_pid=command.get('editor_pid')
            path = pathlib.Path(command['buffer_path'])
            info = path.lstat()
            if not stat.S_ISREG(info.st_mode) or info.st_uid!=os.getuid() or info.st_size>16*1024*1024:
                self.editor_error='source_has_draft: native draft cannot be proved empty'
            elif path.read_bytes():
                self.editor_error='source_has_draft: native editor has unsent content; nothing was changed'
            else:
                self.editor_ready=True
            self.condition.notify_all()
            if not self.editor_error:
                self.condition.wait_for(lambda:self.release,timeout=35)
            self.armed=False
        self.editor_done.set()
        return {'ok':not bool(self.editor_error),'preserve':bool(self.editor_error)}

    def release_and_wait_frontend(self, config):
        before=app_control(config,'inspect')['output_seq']
        self.release_editor()
        if not self.editor_done.wait(timeout=5) or not wait_editor_exit(self.editor_pid):
            raise RuntimeError('native_context_unavailable: native editor did not release')
        deadline=time.monotonic()+3
        while time.monotonic()<deadline:
            state=app_control(config,'inspect')
            if not state['queue_unchanged']:
                raise RuntimeError('source_changed: native input changed during editor release')
            if state['output_seq']!=before:return
            threading.Event().wait(.02)
        raise RuntimeError('native_context_unavailable: native frontend did not repaint after editor release')

    def request(self, command):
        config=self.configuration()
        if not hmac.compare_digest(str(command.get('token','')),config['token']):
            raise RuntimeError('context_not_authorized: native capability mismatch')
        self.monitor_source(config)
        action=command.get('action')
        if action=='editor':
            return self.handle_editor(config,command)
        if action=='ready':
            self.snapshot(config)
            self.release_and_wait_frontend(config)
            return {'ok':True,'ready':True}
        if action=='snapshot':
            return self.snapshot(config)
        if action=='release':
            self.release_editor()
            return {'ok':True}
        if action=='close':
            value=self.capture(config)
            digest=hashlib.sha256(json.dumps(value,sort_keys=True,separators=(',',':')).encode()).hexdigest()
            if digest != command.get('snapshot_id') or digest != self.snapshot_id:
                self.release_editor()
                raise RuntimeError('source_changed: native context changed before close')
            self.release_and_wait_frontend(config)
            app_control(config,'exit')
            return {'ok':True,'closing':True}
        raise RuntimeError('native_context_unavailable: unknown native context operation')

    def serve_client(self, client):
        with self.condition:self.clients+=1
        try:
            client.settimeout(40)
            data=bytearray()
            while b'\n' not in data:
                chunk=client.recv(4096)
                if not chunk:return
                data.extend(chunk)
                if len(data)>8192:return
            try:response=self.request(json.loads(data.split(b'\n')[0]))
            except Exception as error:
                text=str(error);code,separator,message=text.partition(': ')
                response={'ok':False,'error':{'code':code if separator else 'native_context_unavailable',
                                            'message':message if separator else 'Native source operation failed; inspect the original pane'}}
            client.sendall((json.dumps(response)+'\n').encode())
        finally:
            client.close()
            with self.condition:self.clients-=1;self.condition.notify_all()
            self.slots.release()

    def run(self):
        config=self.configuration()
        listener=socket.socket(socket.AF_UNIX)
        listener.bind(config['endpoint'])
        os.chmod(config['endpoint'],0o600)
        listener.listen(4)
        endpoint_info=os.lstat(config['endpoint'])
        wake_read,self.wake=socket.socketpair()
        selector=selectors.DefaultSelector()
        selector.register(listener,selectors.EVENT_READ,'listener')
        selector.register(wake_read,selectors.EVENT_READ,'wake')
        deadline=time.monotonic()+30
        self.monitor_source(config)
        try:
            while not self.stopped.is_set():
                timeout=None if self.owner_pid is not None else max(0,deadline-time.monotonic())
                events=selector.select(timeout)
                if not events and self.owner_pid is None:break
                for key,_ in events:
                    if key.data=='wake':
                        wake_read.recv(4096)
                        continue
                    client,_=listener.accept()
                    if not self.slots.acquire(blocking=False):client.close();continue
                    threading.Thread(target=self.serve_client,args=(client,),daemon=True).start()
        finally:
            selector.close();listener.close();self.release_editor()
            # An exit event can arrive while close is flushing its acknowledgement.
            with self.condition:self.condition.wait_for(lambda:self.clients==0,timeout=5)
            wake_read.close();self.wake.close()
            try:
                current=os.lstat(config['endpoint'])
                if (current.st_dev,current.st_ino)==(endpoint_info.st_dev,endpoint_info.st_ino):os.unlink(config['endpoint'])
            except FileNotFoundError:pass
            directory=pathlib.Path(self.lease).parent
            for name in ['source.py','launch.sh','editor.sh','lease.json','service.log']:
                try:(directory/name).unlink()
                except FileNotFoundError:pass
            try:directory.rmdir()
            except OSError:pass


def parse_arguments(arguments=None):
    parser=argparse.ArgumentParser();parser.add_argument('mode',choices=['server','editor']);parser.add_argument('--lease',required=True);parser.add_argument('buffer',nargs='?')
    # Native programs append the editor filename after configured options.
    # parse_args differs here between system Python 3.9 and newer releases.
    return parser.parse_intermixed_args(arguments)


def main():
    args=parse_arguments()
    if args.mode=='server':ContextServer(args.lease).run()
    else:raise SystemExit(editor(args.lease,args.buffer))

if __name__=='__main__':main()
