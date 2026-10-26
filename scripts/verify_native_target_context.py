"""Qualify a target's persisted context through its installed native loader.
Runs on a private copy so the interactive source session is never modified.
"""
import json
import os
import pathlib
import shutil
import subprocess
import tempfile


def qualify(target, path, cwd, native_env, fixture, required, excluded):
    root=pathlib.Path(tempfile.mkdtemp(prefix='context-proof-',dir=pathlib.Path(cwd)))
    env=native_env.copy()
    for key in list(env):
        if key.startswith('HERDR_'):env.pop(key)
    source=pathlib.Path(path)
    sid=source.stem
    if target=='pi':
        global_root=subprocess.check_output(['npm','root','-g'],text=True).strip()
        module=pathlib.Path(global_root)/'@earendil-works/pi-coding-agent/dist/index.js'
        script='''import {SessionManager,convertToLlm} from MODULE;
const session=SessionManager.open(process.argv[1]);
const context=convertToLlm(session.buildSessionContext().messages);
console.log(JSON.stringify(context));'''.replace('MODULE',json.dumps(module.as_uri()))
        copy=root/source.name;shutil.copyfile(source,copy)
        result=subprocess.run(['node','--input-type=module','-e',script,str(copy)],env=env,cwd=cwd,capture_output=True,text=True,timeout=20)
        assert result.returncode==0,result.stderr
        captured=json.loads(result.stdout)
    else:
        home=root/'home';home.mkdir()
        if target=='codex':
            env['CODEX_HOME']=str(home)
            rows=[json.loads(line) for line in source.read_text().splitlines()]
            sid=rows[0]['payload']['id']
            dest=home/'sessions/2026/10/26'/('rollout-2026-10-26T00-00-00-'+sid+'.jsonl')
            dest.parent.mkdir(parents=True);shutil.copyfile(source,dest)
            (home/'config.toml').write_text('model_provider="capture"\nmodel="fixture-model"\n[model_providers.capture]\nname="Local capture"\n'+f'base_url="{fixture.url}/v1"\nwire_api="responses"\nrequires_openai_auth=false\nsupports_websockets=false\n')
            command=['codex','exec','--skip-git-repo-check','resume',sid,'Without tools, reply CAPTURED.']
        else:
            env['CLAUDE_CONFIG_DIR']=str(home)
            directory=home/'projects'/''.join(c if c.isascii() and (c.isalnum() or c=='-') else '-' for c in str(pathlib.Path(cwd).resolve()))
            directory.mkdir(parents=True);shutil.copyfile(source,directory/source.name)
            command=['claude','--bare','--print','--tools','','--output-format','json','--resume',sid,'Without tools, reply CAPTURED.']
        start=len(fixture.requests)
        result=subprocess.run(command,env=env,cwd=cwd,stdin=subprocess.DEVNULL,capture_output=True,text=True,timeout=30)
        assert result.returncode==0,result.stderr[-2000:]+result.stdout[-1000:]
        calls=[r['payload'] for r in fixture.requests[start:] if ('/responses' in r['path'] or '/messages' in r['path']) and 'count_tokens' not in r['path']]
        assert calls,'native target did not send a model request'
        captured=[r.get('input',r.get('messages')) for r in calls]
    serialized=json.dumps(captured)
    for marker in required:assert marker in serialized,'native active context lost '+marker[:60]
    for marker in excluded:assert marker not in serialized,'native active context resurrected '+marker[:60]
    (root/'native-context.json').write_text(json.dumps(captured,indent=2))
    return {'target':target,'native_loader_verified':True,'request_or_projection':str(root/'native-context.json')}
