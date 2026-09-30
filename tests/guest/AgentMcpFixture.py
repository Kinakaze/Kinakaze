"""Small local JSON-RPC stdio fixture for real agent MCP clients."""
import hashlib
import json
from pathlib import Path
import sys


root = Path(__file__).resolve().parent
for line in sys.stdin:
    request = json.loads(line)
    if 'id' not in request:
        continue
    method = request.get('method')
    if method == 'initialize':
        result = dict(protocolVersion=request['params']['protocolVersion'], capabilities=dict(tools={}),
                      serverInfo=dict(name='kinakaze-agent-fixture', version='1.0'))
    elif method == 'ping':
        result = {}
    elif method == 'tools/list':
        result = dict(tools=[
            dict(name='echo', description='Echo fixture text unchanged.', inputSchema=dict(type='object',
                 properties=dict(text=dict(type='string')), required=['text'], additionalProperties=False)),
            dict(name='inspect', description='Read the generated daily.py fixture.', inputSchema=dict(type='object', properties={}))])
    elif method == 'tools/call':
        name = request['params']['name']
        if name == 'echo':
            text = 'MCP_ECHO:' + request['params']['arguments']['text']
        elif name == 'inspect':
            source = (root / 'daily.py').read_bytes()
            text = json.dumps(dict(source=source.decode('utf-8'), sha256=hashlib.sha256(source).hexdigest()), ensure_ascii=False)
        else:
            raise ValueError('unknown fixture tool: ' + str(name))
        with (root / 'mcp-invocations.jsonl').open('a', encoding='utf-8') as log:
            log.write(json.dumps(dict(name=name, text=text), ensure_ascii=False) + '\n')
        result = dict(content=[dict(type='text', text=text)], isError=False)
    else:
        print(json.dumps(dict(jsonrpc='2.0', id=request['id'], error=dict(code=-32601, message='Unknown method'))), flush=True)
        continue
    print(json.dumps(dict(jsonrpc='2.0', id=request['id'], result=result), ensure_ascii=False), flush=True)
