"""Bounded public JSON API transport shared by native qualification scripts."""
import json
import socket


def request(endpoint, method, params=None, timeout=45):
    with socket.socket(socket.AF_UNIX) as stream:
        stream.settimeout(timeout)
        stream.connect(str(endpoint))
        stream.sendall((json.dumps({'id':method,'method':method,'params':params or {}})+'\n').encode())
        data=bytearray()
        while b'\n' not in data:
            chunk=stream.recv(65536)
            if not chunk:raise RuntimeError(method+': API disconnected')
            data.extend(chunk)
            if len(data)>256*1024*1024:raise RuntimeError(method+': API response exceeds bound')
        return json.loads(data.split(b'\n')[0])


def call(endpoint, method, params=None, timeout=45):
    response=request(endpoint,method,params,timeout)
    if 'error' in response:raise RuntimeError(method+': '+str(response['error']))
    return response['result']
