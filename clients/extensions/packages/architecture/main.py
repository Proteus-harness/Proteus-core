import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Analytics, stack, text, button, select, code


def graph(snapshot):
    nodes = [{"id":"config","label":snapshot["profile"] or "Профиль","subtitle":"Конфигурация","tone":"accent","active":True,"data":{key:value for key,value in snapshot.items() if key not in ("modules","slots","tools","edges")}},
             {"id":"tools","label":"Инструменты","subtitle":f"{sum(tool['registered'] for tool in snapshot['tools'])} зарегистрировано","tone":"success","active":True,"data":{}}]
    for slot in sorted(snapshot["slots"],key=lambda slot:(slot["order"],slot["id"])):
        nodes.append({"id":"slot:"+slot["id"],"label":slot["title"] or slot["id"],"subtitle":slot.get("active_module") or "Модуль не выбран","tone":"warning","active":slot["active_module"] is not None,"data":slot})
    for module in snapshot["modules"]:
        nodes.append({"id":f"module:{module['slot']}:{module['id']}","label":module["id"],"subtitle":module["slot"]+(" · выбран" if module["active"] else " · доступен"),"tone":"accent","active":module["active"],"data":module})
    for tool in snapshot["tools"]:
        nodes.append({"id":"tool:"+tool["name"],"label":tool["name"],"subtitle":"Включён" if tool["enabled"] and tool["registered"] else "Отключён" if tool["registered"] else "Не зарегистрирован","tone":"success","active":tool["enabled"] and tool["registered"],"data":tool})
    ids={node["id"] for node in nodes}
    for edge in snapshot["edges"]:
        for identity in (edge["from"],edge["to"]):
            if identity not in ids:
                nodes.append({"id":identity,"label":identity,"subtitle":"Связь из snapshot","tone":"muted","active":True,"data":{}})
                ids.add(identity)
    return nodes


class Architecture(Analytics):
    poll_seconds = None

    def __init__(self):
        super().__init__()
        self.tab = "map"
        self.scope = "assembly"
        self.inactive = False
        self.query = ""
        self.selected = None
        self.snapshot = None

    def event(self, event):
        if event in ("session_opened","run_settled"): self.snapshot=None

    def render(self, view, surface, argument):
        if not self.session:
            self.show(view,text("Нет активной сессии.","muted"))
            return
        if self.snapshot is None: self.snapshot=self.api("/inspect/topology")
        snapshot=self.snapshot
        header=[text("Архитектура"),text(f"Профиль {snapshot['profile']} · ревизия {snapshot['module_epoch']} · {snapshot['cwd']}","muted"),
                stack(button("Карта","tab","map"),button("Каталог","tab","catalog"),button("Обновить","refresh"),button("Копировать Mermaid","mermaid"),direction="row"),
                {"type":"input","id":"query","value":self.query,"placeholder":"Найти модуль, слот или инструмент…","action":"query"}]
        nodes=graph(snapshot)
        if self.tab=="catalog":
            rows=[{"id":node["id"],"label":node["label"],"detail":node["subtitle"],"selected":node["id"]==self.selected,"tone":node["tone"]} for node in nodes if self.query.lower() in str(node).lower()]
            content={"type":"tree","id":"catalog","rows":rows,"action":"node"}
        else:
            header.append(stack(select("scope",{"assembly":"Сборка","modules":"Модули","tools":"Инструменты"}[self.scope],[("assembly","Сборка"),("modules","Модули"),("tools","Инструменты")],self.scope,"scope"),
                         select("inactive","Все реализации" if self.inactive else "Активные",[(False,"Активные"),(True,"Все реализации")],self.inactive,"inactive"),direction="row"))
            shown=[]
            for node in nodes:
                kind=node["id"].split(":")[0]
                if self.scope=="assembly" and kind=="tool": continue
                if self.scope=="modules" and kind not in ("slot","module"): continue
                if self.scope=="tools" and kind not in ("tools","tool","module"): continue
                if not self.inactive and not node["active"] and kind in ("module","tool"): continue
                shown.append(node)
            columns={"config":0,"slot":1,"module":2,"tools":3,"tool":4}
            counts={}
            positioned=[]
            for node in shown:
                column=columns.get(node["id"].split(":")[0],0)
                row=counts.get(column,0);counts[column]=row+1
                match=not self.query or self.query.lower() in str(node).lower()
                positioned.append({"id":node["id"],"label":node["label"],"subtitle":node["subtitle"],"tone":node["tone"],"selected":node["id"]==self.selected,
                                   "dimmed":not match or not node["active"],"x":32+column*224,"y":32+row*80})
            ids={node["id"] for node in positioned}
            edges=[{"from":edge["from"],"to":edge["to"]} for edge in snapshot["edges"] if edge["from"] in ids and edge["to"] in ids]
            content={"type":"graph","id":"topology-"+self.scope+str(self.inactive),"nodes":positioned,"edges":edges,"action":"select"}
        body=header+[content]
        selected=next((node for node in nodes if node["id"]==self.selected),None)
        if selected:
            links=[edge for edge in snapshot["edges"] if self.selected in (edge["from"],edge["to"])]
            body += [text(selected["label"]),code("node-detail",{"node":selected["data"],"links":links},220)]
        for warning in snapshot["warnings"]: body.append(text(warning["message"],"danger" if warning["severity"]=="error" else "warning"))
        self.show(view,{"type":"fill","children":body})

    def action(self, view, action, value):
        if action in ("tab","scope","inactive","query"): setattr(self,action,value)
        elif action=="refresh": self.snapshot=None
        elif action=="node": self.selected=value["id"]
        elif action=="select": self.selected=value if self.selected!=value else None
        elif action=="mermaid": self.send({"type":"clipboard","text":self.api("/inspect/topology.mmd",text=True)})


if __name__ == "__main__":
    Architecture().run()
