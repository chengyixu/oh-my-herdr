"""Task-owned native API fixture used to observe actual outgoing model context.
Never reads or changes the operator's credentials or provider configuration.
"""
import http.server
import json
import threading
import os
import pathlib
import shutil
import shlex
import subprocess

class NativeFixture:
    def __init__(self):
        self.requests = []
        owner = self
        class Handler(http.server.BaseHTTPRequestHandler):
            def log_message(self, *args):
                pass
            def do_GET(self):
                self.send_response(200)
                self.send_header('Content-Type', 'application/json')
                self.end_headers()
                self.wfile.write(b'{"data":[]}')
            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers['Content-Length'])))
                owner.requests.append({'path': self.path, 'payload': body})
                if 'count_tokens' in self.path:
                    data=b'{"input_tokens":1000}'
                    self.send_response(200); self.send_header('Content-Type','application/json')
                elif '/messages' in self.path:
                    msg={'id':'msg_fixture','type':'message','role':'assistant','model':body.get('model'),'content':[],
                         'stop_reason':None,'stop_sequence':None,'usage':{'input_tokens':1,'output_tokens':0}}
                    events=[('message_start',{'type':'message_start','message':msg}),
                            ('content_block_start',{'type':'content_block_start','index':0,'content_block':{'type':'text','text':''}}),
                            ('content_block_delta',{'type':'content_block_delta','index':0,'delta':{'type':'text_delta','text':'CAPTURED'}}),
                            ('content_block_stop',{'type':'content_block_stop','index':0}),
                            ('message_delta',{'type':'message_delta','delta':{'stop_reason':'end_turn','stop_sequence':None},'usage':{'output_tokens':1}}),
                            ('message_stop',{'type':'message_stop'})]
                    data=''.join('event: '+name+'\ndata: '+json.dumps(event)+'\n\n' for name,event in events).encode()
                    self.send_response(200); self.send_header('Content-Type','text/event-stream')
                else:
                    item={'id':'msg_fixture','type':'message','role':'assistant','status':'completed',
                          'content':[{'type':'output_text','text':'CAPTURED','annotations':[]}]}
                    events=[{'type':'response.created','response':{'id':'resp_fixture','status':'in_progress','output':[]}},
                            {'type':'response.output_item.added','output_index':0,'item':{'id':'msg_fixture','type':'message','role':'assistant','content':[]}},
                            {'type':'response.output_text.delta','output_index':0,'content_index':0,'item_id':'msg_fixture','delta':'CAPTURED'},
                            {'type':'response.output_item.done','output_index':0,'item':item},
                            {'type':'response.completed','response':{'id':'resp_fixture','status':'completed','output':[item],
                                'usage':{'input_tokens':1,'output_tokens':1,'total_tokens':2}}}]
                    data=''.join('event: '+e['type']+'\ndata: '+json.dumps(e)+'\n\n' for e in events).encode()
                    self.send_response(200); self.send_header('Content-Type','text/event-stream')
                self.send_header('Content-Length',str(len(data))); self.end_headers(); self.wfile.write(data)
        self.server=http.server.ThreadingHTTPServer(('127.0.0.1',0),Handler)
        threading.Thread(target=self.server.serve_forever,daemon=True).start()
        self.url=f'http://127.0.0.1:{self.server.server_port}'
    def close(self):
        self.server.shutdown(); self.server.server_close()

    def configure_environment(self, env, base, binary, extension):
        native=base/"native"; native.mkdir()
        codex_home=native/"codex"; codex_home.mkdir()
        claude_home=native/"claude"; claude_home.mkdir()
        pi_home=native/"pi"; pi_home.mkdir()
        (pi_home/"settings.json").write_text(json.dumps({"defaultProvider":"openai","defaultModel":"gpt-4o","quietStartup":True}))
        (pi_home/"extensions").mkdir()
        shutil.copyfile(extension,pi_home/"extensions/herdr-agent-state.ts")
        (codex_home/"config.toml").write_text(
            'model_provider="capture"\nmodel="fixture-model"\n[features]\nhooks=true\n[model_providers.capture]\nname="Task-owned local capture"\n'
            f'base_url="{self.url}/v1"\nwire_api="responses"\nrequires_openai_auth=false\nsupports_websockets=false\n')
        (claude_home/".claude.json").write_text(json.dumps({"hasCompletedOnboarding":True,"bypassPermissionsModeAccepted":True,"customApiKeyResponses":{"approved":["task-owned-fixture"],"rejected":[]},
            "projects":{str(base.resolve()):{"hasTrustDialogAccepted":True,"hasCompletedProjectOnboarding":True}}}))
        env.update(CODEX_HOME=str(codex_home),CLAUDE_CONFIG_DIR=str(claude_home),PI_CODING_AGENT_DIR=str(pi_home),
                   ANTHROPIC_BASE_URL=self.url,ANTHROPIC_API_KEY="task-owned-fixture",ANTHROPIC_MODEL="claude-sonnet-4-6",
                   CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC="1",NO_PROXY="127.0.0.1,localhost")
        for key in ["ANTHROPIC_AUTH_TOKEN","CLAUDE_CODE_OAUTH_TOKEN","OPENAI_API_KEY","CODEX_API_KEY"]:env.pop(key,None)
        # Vetted task-owned hooks only; native Codex 0.164 has a separate hook-trust prompt.
        fixture_bin=base/"fixture-bin"; fixture_bin.mkdir()
        native_codex=shutil.which("codex")
        (fixture_bin/"codex").write_text('#!/bin/sh\nexec '+shlex.quote(native_codex)+' --dangerously-bypass-hook-trust "$@"\n')
        (fixture_bin/"codex").chmod(0o700)
        env["PATH"]=str(fixture_bin)+os.pathsep+env["PATH"]
        for integration in ["codex","claude"]:
            install=subprocess.run([str(binary),"integration","install",integration],env=env,capture_output=True,text=True,timeout=15)
            assert install.returncode==0,install.stderr
