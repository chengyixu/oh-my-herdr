"""Qualify fresh, native-created Claude/Codex histories through real pane switches.
Uses only a task-owned server, native homes, and local HTTP protocol fixture.
"""
import argparse
import hashlib
import json
import os
import pathlib
import subprocess
import tempfile
import time
import uuid
from native_protocol_fixture import NativeFixture
from native_test_client import call, request
from verify_native_target_context import qualify


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary',type=pathlib.Path,required=True)
    parser.add_argument('--source',choices=['codex','claude'],required=True)
    parser.add_argument('--output',type=pathlib.Path,required=True)
    options=parser.parse_args()
    output=options.output.resolve();output.mkdir(parents=True,exist_ok=True)
    base=pathlib.Path(tempfile.mkdtemp(prefix='ohfresh-',dir='/tmp')).resolve()
    binary=options.binary.resolve()
    fixture=NativeFixture()
    env={k:v for k,v in os.environ.items() if not k.startswith('HERDR_')}
    name='fresh-'+uuid.uuid4().hex[:8]
    env.update(HERDR_ENV='1',HERDR_SESSION=name,HERDR_CONFIG_PATH=str(base/'config.toml'),
               XDG_CONFIG_HOME=str(base/'config'),XDG_STATE_HOME=str(base/'state'),SHELL='/bin/sh')
    extension=pathlib.Path(__file__).resolve().parents[1]/'src/integration/assets/pi/herdr-agent-state.ts'
    fixture.configure_environment(env,base,binary,extension)
    (base/'config.toml').write_text('onboarding=false\n[experimental]\nallow_nested=true\n[update]\nversion_check=false\nmanifest_check=false\n')
    log=(base/'server.log').open('w')
    server=subprocess.Popen([str(binary),'--session',name,'server'],env=env,stdin=subprocess.DEVNULL,stdout=log,stderr=log)
    endpoint=None
    report={'source':options.source,'base':str(base),'transitions':[]}
    oldest='FRESH-OLDEST-'+uuid.uuid4().hex
    newest='FRESH-LATEST-'+uuid.uuid4().hex
    markers=[oldest,newest]
    def api(method,params=None,timeout=45):return call(endpoint,method,params,timeout)
    def wait_reply(start):
        deadline=time.monotonic()+25
        while time.monotonic()<deadline:
            screen=api('pane.read',{'pane_id':pane,'source':'detection','lines':80})['read']['text']
            if len(fixture.requests)>start and ('• CAPTURED' in screen or '⏺ CAPTURED' in screen):return
            time.sleep(.1)
        raise AssertionError('the native TUI did not receive the local fixture reply')
    try:
        deadline=time.monotonic()+15
        while time.monotonic()<deadline:
            sessions=json.loads(subprocess.check_output([str(binary),'session','list','--json'],env=env,text=True,timeout=5))['sessions']
            item=next((s for s in sessions if s['name']==name and s['running']),None)
            if item:endpoint=pathlib.Path(item['socket_path']);break
            time.sleep(.1)
        assert endpoint,'task-owned server did not start'
        root=api('workspace.create',{'cwd':str(base),'focus':True})
        pane=root['root_pane']['pane_id'];terminal=root['root_pane']['terminal_id']
        report.update(pane=pane,terminal=terminal)
        api('agent.profile.create',{'role':'fresh-native','harness':options.source,'native_cwd':str(base),'instructions':'Do not replay historical actions.'})
        for target in ['codex','pi','claude']:api('integration.harness.set',{'target':target,'active':True})
        deadline=time.monotonic()+15
        while True:
            started=request(endpoint,'agent.start',{'name':'fresh-native','kind':options.source,'pane_id':pane,'args':[],'timeout_ms':25000})
            if 'error' not in started:break
            assert started['error']['code']=='agent_pane_busy' and time.monotonic()<deadline,started
            time.sleep(.1)
        api('agent.ready.wait',{'target':pane,'timeout_ms':30000})
        for marker in markers:
            start=len(fixture.requests)
            api('agent.prompt',{'target':pane,'text':'Without tools, reply CAPTURED. Retain '+marker})
            wait_reply(start)
            api('agent.ready.wait',{'target':pane,'timeout_ms':15000})
        source=api('agent.get',{'target':pane})['agent']['agent_session']
        instance=api('agent.harness.prepare',{'target':pane})['instance_id']
        report.update(source_session=source,instance=instance,pane=pane,terminal=terminal)
        process=api('pane.process_info',{'pane_id':pane})['process_info']
        targets=['pi','claude','codex'] if options.source=='codex' else ['codex','pi','claude']
        report['native_context_proofs']=[]
        for target in targets:
            api('pane.send_text',{'pane_id':pane,'text':'UNSENT-NATIVE-DRAFT'})
            refused=request(endpoint,'agent.harness.replace',{'instance_id':instance,'harness':target},150)
            assert refused.get('error',{}).get('code')=='source_has_draft',refused
            deadline=time.monotonic()+5
            while time.monotonic()<deadline:
                draft=api('pane.read',{'pane_id':pane,'source':'detection','lines':20})['read']['text']
                if 'UNSENT-NATIVE-DRAFT' in draft:break
                time.sleep(.05)
            assert 'UNSENT-NATIVE-DRAFT' in draft,draft
            api('pane.send_keys',{'pane_id':pane,'keys':['Ctrl+c']})
            result=api('agent.harness.replace',{'instance_id':instance,'harness':target},150)
            report['transitions'].append({'target':target,'result':result})
            assert result['pane']['pane_id']==pane and result['pane']['terminal_id']==terminal,result
            after=api('pane.process_info',{'pane_id':pane})['process_info']
            assert after['shell_pid']==process['shell_pid'],after
            assert len(api('pane.list')['panes'])==1
            registry=json.loads((endpoint.parent/'agents.json').read_text())
            recovery=registry['native_harness_recovery'][instance]
            native=pathlib.Path(recovery['target_path']).read_text()
            assert all(marker in native for marker in markers),'target native history lost a marker'
            report['native_context_proofs'].append(qualify(target,recovery['target_path'],base,env,fixture,markers,[]))
        report['success']=True
    except Exception as error:
        report.update(success=False,error=str(error))
        if endpoint:
            try:report['screen']=api('pane.read',{'pane_id':pane,'source':'detection','lines':80})['read']['text']
            except Exception:pass
    finally:
        if endpoint:
            try:api('server.stop');server.wait(timeout=10)
            except Exception:
                if server.poll() is None:server.terminate();server.wait(timeout=5)
        elif server.poll() is None:server.terminate();server.wait(timeout=5)
        fixture.close();log.close()
        (base/'captured-requests.json').write_text(json.dumps(fixture.requests,indent=2))
        (output/'result.json').write_text(json.dumps(report,indent=2)+'\n')
        print(json.dumps({k:report.get(k) for k in ['success','source','error','base','pane','terminal']},indent=2))
    return 0 if report.get('success') else 1

if __name__=='__main__':raise SystemExit(main())
