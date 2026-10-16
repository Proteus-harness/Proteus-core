"""Exercise every shipped process using synthetic public-API data.

--wire emits the actual messages for the Rust protocol conformance test.
"""

import contextlib
import http.server
import json
from pathlib import Path
import queue
import subprocess
import sys
import threading
import urllib.parse

PACKAGES = Path(__file__).resolve().parents[1] / "packages"


def state():
    return {
        "connected":True,"context":{"used":14021,"max":200000,"trigger":180000},
        "execution":{"active":None,"last":None},"pending":{"approvals":[],"user_inputs":[],"queued_user_messages":[]},
        "activity":{"status":"idle"},
        "config":{"profile":"fixture","permission_mode":"normal","model":{"provider":"fixture","name":"gpt-6-astra","label":"Test model"},
                  "model_options":[{"provider":"fixture","name":"gpt-6-astra","label":"Test model","hidden":False,"reasoning_efforts":["low","high"],"default_reasoning_effort":"high"}],
                  "reasoning":{"enabled":True,"effort":None,"effort_options":["low","high"]},
                  "registered_tools":[{"name":"probe"}],"modules":[{"slot":"model","id":"fake"}]},
    }


def entry():
    return {"role":"assistant","tool":{"name":"update_plan","args":{"plan":[{"step":"One","status":"completed"},{"step":"Two","status":"pending"}]},"effective_args":None,"status":"completed"},"subagent":None}


def usage():
    request={"exchange_id":"request-1","turn_id":"turn-1","model":{"provider":"fixture","model":"gpt-6-astra"},"origin":"direct",
             "started_at_ms":1000,"finished_at_ms":2000,"status":"completed","finish_reason":"stop",
             "usage":{"input_tokens":10000,"output_tokens":1000,"cached_input_tokens":2000,"cache_creation_input_tokens":None,"reasoning_output_tokens":400},
             "message_count":3,"tool_count":1,"reasoning_effort":"high","max_output_tokens":None}
    return {"session_id":"session-fixture","revision":1,"latest_turn_id":"turn-1","requests":[request]}


def topology():
    return {"profile":"fixture","cwd":"/workspace","config_path":None,"config_files":[],"module_epoch":1,"permission_mode":"normal","model":None,
            "slots":[{"id":"model","title":"Модель","responsibility":"Select model","active_module":"fake","required":True,"category":"backend","order":1}],
            "modules":[{"id":"fake","slot":"model","active":True,"source":{"kind":"process"},"version":"1","api_version":"v12","capabilities":[],"description":"Fixture"}],
            "tools":[{"name":"probe","description":"Test tool","safety":"read_only","source":"fixture","owner":None,"enabled":True,"runtime_managed":False,"registered":True,"input_schema":{"type":"object"}}],
            "edges":[{"from":"config","to":"slot:model","kind":"configures","label":None},{"from":"slot:model","to":"module:model:fake","kind":"selects","label":None},{"from":"tools","to":"tool:probe","kind":"contains","label":None}],"warnings":[],"hooks":[]}


class Api(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if self.headers.get("Authorization") != "Bearer fixture":
            self.send_error(403)
            return
        parsed=urllib.parse.urlsplit(self.path)
        query=urllib.parse.parse_qs(parsed.query)
        path=parsed.path
        if path=="/sessions": data=[{"session_dir":"/session","preview":"Synthetic session","message_count":2}]
        elif path=="/usage": data=usage()
        elif path=="/context": data={"history":{"estimated_tokens":100},"latest_usage":{"estimated_input_tokens":100,"max_input_tokens":200000,"actual":usage()["requests"][0]["usage"],"categories":[{"name":"history","tokens":100,"source":"estimate"}]},"diagnostics":[]}
        elif path=="/model/quota": data={"observed_at":1700000000,"buckets":[{"id":"main","windows":[{"id":"week","duration_seconds":604800,"used_percent":80}]}]}
        elif path=="/workspace/changes": data={"repository":True,"entries":[{"path":"sample.py","status":"modified"}],"truncated":False}
        elif path=="/workspace/list":
            folder=query.get("path",[""])[0]
            data={"path":folder,"entries":[{"name":"sample.py","path":"sample.py","kind":"file"}],"truncated":False}
        elif path=="/workspace/search": data={"query":query["q"][0],"entries":[{"name":"sample.py","path":"sample.py","kind":"file"}],"truncated":False}
        elif path=="/workspace/file": data={"path":"sample.py","size":12,"kind":"text","text":"print('ok')\n"}
        elif path=="/workspace/diff": data={"path":"sample.py","kind":"text","patch":"+print('ok')\n"}
        elif path=="/inspect/topology": data=topology()
        elif path=="/inspect/topology.mmd":
            self.send_response(200);self.end_headers();self.wfile.write(b"graph LR\n  config --> model\n");return
        elif path=="/analysis":
            request=usage()["requests"][0]
            steps=[{"sequence":1,"started_at_ms":1000,"finished_at_ms":2000,"data":{"kind":"model","request":{"model":request["model"]},"origin":"direct","response":{"finish_reason":"stop","usage":request["usage"]},"failure":None,"messages":[]}},
                   {"sequence":2,"started_at_ms":2000,"finished_at_ms":2100,"data":{"kind":"tool","call":{"name":"probe","args":{}},"result":{"ok":True,"text":"ok"},"resolution":None,"approval_reason":None}}]
            data={"session_id":"session-fixture","revision":1,"turns":[{"turn_id":"turn-1","number":1,"prompt_preview":"Synthetic task","status":"success","started_at_ms":1000,"finished_at_ms":2100}],
                  "selected":{"turn_id":"turn-1","task":{"input":"Synthetic task"},"module_epoch":1,"config_snapshot":{"fixture":True},"steps":steps,"output":None,"error":None}}
        else:
            self.send_error(404);return
        self.send_response(200);self.send_header("Content-Type","application/json");self.end_headers();self.wfile.write(json.dumps(data).encode())

    def log_message(self,*_): pass


class Process:
    def __init__(self,package,origin):
        self.manifest=json.loads((package/"extension.json").read_text())
        self.process=subprocess.Popen(self.manifest["command"],cwd=package,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=subprocess.PIPE,text=True)
        self.messages=queue.Queue()
        self.wire=[]
        def read():
            for line in self.process.stdout:
                message=json.loads(line);self.wire.append(message);self.messages.put(message)
            self.messages.put(None)
        self.reader=threading.Thread(target=read,daemon=True);self.reader.start()
        self.send({"type":"hello","protocol":2,"workspace":"/workspace","server":{"origin":origin,"token":"fixture"}})
        if self.manifest.get("session_events"):
            self.send({"type":"session_snapshot","session_dir":"/session","state":state(),"entries":[{"role":"user","tool":None,"subagent":None},entry()]})

    def send(self,message):
        self.process.stdin.write(json.dumps(message)+"\n");self.process.stdin.flush()

    def wait(self,predicate):
        while True:
            message=self.messages.get(timeout=10)
            if message is None: raise AssertionError("Extension exited: "+self.process.stderr.read())
            if message.get("type")=="view" and message["root"].get("tone")=="danger":
                raise AssertionError(f"{self.manifest['id']}: {message['root']}")
            if predicate(message): return message

    def render(self,surface):
        argument="1" if self.manifest["id"]=="terminal" else "sample.py"
        view="document:"+argument if surface=="document" else surface
        self.send({"type":"render","surface":surface,"view":view,"session_dir":"/session","argument":argument if surface=="document" else None})
        return self.wait(lambda message:message.get("view")==view and message["type"] in ("view","badge"))

    def action(self,view,action,value):
        self.send({"type":"action","view":view,"action":action,"value":value,"sequence":1})

    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=10);self.reader.join(timeout=5)
        error=self.process.stderr.read()
        self.process.stdout.close();self.process.stderr.close()
        if self.process.returncode or error: raise AssertionError(f"{self.manifest['id']}: {error}")


def exercise(packages=PACKAGES):
    wire=[]
    with contextlib.ExitStack() as stack:
        server=http.server.ThreadingHTTPServer(("127.0.0.1",0),Api)
        threading.Thread(target=server.serve_forever,daemon=True).start()
        stack.callback(server.server_close);stack.callback(server.shutdown)
        for package in sorted(packages.iterdir()):
            if not (package/"extension.json").exists():continue
            process=Process(package,f"http://127.0.0.1:{server.server_port}")
            try:
                for surface in process.manifest["surfaces"]:process.render(surface)
                identity=package.name
                if identity=="mode":
                    process.action("popup","mode","plan")
                    command=process.wait(lambda message:message["type"]=="set_session")
                    assert command["session_dir"]=="/session" and command["setting"]["value"]=="plan"
                elif identity=="model":
                    process.action("popup","effort",0)
                    assert process.wait(lambda message:message["type"]=="set_session")["setting"]["value"]=="low"
                elif identity=="files":
                    process.action("panel","search-focus",None)
                    assert process.wait(lambda message:message["type"]=="focus")["control"]=="search"
                    process.action("panel","search","sample")
                    process.wait(lambda message:message.get("view")=="panel" and message.get("sequence")==1)
                    process.action("panel","file",{"id":"sample.py","event":"open"})
                    assert process.wait(lambda message:message["type"]=="open_document")["argument"]=="sample.py"
                elif identity=="usage":
                    process.action("page","tab","context")
                    process.wait(lambda message:message.get("view")=="page" and message.get("sequence")==1)
                elif identity=="analysis":
                    process.action("page","step",{"id":"2","event":"select"})
                    process.wait(lambda message:message.get("view")=="page" and message.get("sequence")==1)
                elif identity=="architecture":
                    process.action("page","mermaid",None)
                    assert "config --> model" in process.wait(lambda message:message["type"]=="clipboard")["text"]
            finally:
                process.close()
                wire.extend(process.wire)
    return wire


if __name__=="__main__":
    packages = Path(sys.argv[sys.argv.index("--packages")+1]) if "--packages" in sys.argv else PACKAGES
    messages=exercise(packages)
    if "--wire" in sys.argv:
        for message in messages:print(json.dumps(message,ensure_ascii=False))
    else:print(f"12 packages: {len(messages)} real protocol messages checked")
