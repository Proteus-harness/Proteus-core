#!/usr/bin/env python3
"""Synthetic stdio peer: root and descendant retain distinct source identities."""
import json, os, pathlib, sys, uuid
os.chdir(sys.argv[sys.argv.index('--cwd') + 1])

def uid(): return str(uuid.uuid4())
def emit(value): print(json.dumps(value), flush=True)
def response(id, value): emit({'type':'response','id':id,'ok':True,'output':value,'error':None})
def event(thread, turn, value):
    emit({'type':'event','event':{'type':'runtime','envelope':{
        'schema_version':2,'event_id':uid(),'session_id':session,'thread_id':thread,
        'turn_id':turn,'seq':1,'timestamp_ms':0,'event':value}}})
def new_session():
    global session, root, descendant
    session, root, descendant = uid(), uid(), uid()
    event(root, None, {'SessionStarted':{'session_id':session,'cwd':str(pathlib.Path.cwd()),
          'model':None,'session_dir':str(pathlib.Path.cwd()/session)}})
def interact(kind, origin):
    id = uid()
    common = {'cwd':str(pathlib.Path.cwd()),'origin':origin,'seq':1}
    if kind == 'user_input':
        request = dict(common, request_id=id,title=None,questions=[])
    else:
        request = dict(common, approval_id=id,call={'id':'local','name':'read','args':{},
                       'surface':'function','raw_arguments':None},reason='fixture',tool_spec=None,preview=None)
    emit({'type':'event','event':{'type':kind+'_requested','request':request}})
    reply = json.loads(next(sys.stdin))
    assert reply['type'] == kind and reply[('request_id' if kind == 'user_input' else 'approval_id')] == id, reply

new_session()
for line in sys.stdin:
    request = json.loads(line)
    if request['type'] == 'new_session':
        new_session()
        response(request['id'], {'session_id':session,'session_dir':str(pathlib.Path.cwd()/session)})
    elif request['type'] == 'send':
        active = request['id']; turn = uid(); execution = uid(); descendant_turn = uid(); descendant_execution = uid()
        info = {'pid':os.getpid(),'session':session,'root':root,'descendant':descendant,'turn':turn,
                'execution':execution,'descendant_turn':descendant_turn,'descendant_execution':descendant_execution}
        event(root, turn, {'SubagentStarted':{'role':'leaf','description':'leaf','child_thread_id':descendant}})
        for thread, source_turn, name in [(root,turn,'root'),(descendant,descendant_turn,'descendant')]:
            event(thread, source_turn, {'ToolCallRequested':{'call':{'id':name+'-'+turn,'name':'read',
                  'args':info,'surface':'function','raw_arguments':None}}})
        if request['text'] == 'cancel':
            pathlib.Path('ready').touch()
            continue
        for kind in ['user_input','approval']:
            for thread, source_turn, source_execution, label in [(root,turn,execution,'physical-root'),
                (descendant,descendant_turn,descendant_execution,'leaf')]:
                interact(kind, {'execution_id':source_execution,'thread_id':thread,'turn_id':source_turn,'label':label})
            interact(kind, {'execution_id':execution,'label':'standalone'})
            interact(kind, None)
        response(active, {'text':json.dumps(info),'metadata':None})
    elif request['type'] == 'cancel':
        event(descendant, descendant_turn, {'Error':{'message':'descendant settlement'}})
        response(request['target_id'], {'text':'settled','metadata':None})
