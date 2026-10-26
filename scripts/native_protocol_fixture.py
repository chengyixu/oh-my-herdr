"""Task-owned native API fixture used to observe actual outgoing model context.
Never reads or changes the operator's credentials or provider configuration.
"""
import http.server
import json
import threading

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
