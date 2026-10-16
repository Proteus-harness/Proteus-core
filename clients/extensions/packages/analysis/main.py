import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Analytics, stack, text, button, select, metric, code, tokens

TURN_STATUS = {"unsettled":"Нет завершения", "success":"Завершён", "error":"Ошибка", "canceled":"Отменён", "timeout":"Таймаут"}
KINDS = {"model":"Модель", "tool":"Инструмент", "compaction":"Контекст", "hook":"Хуки"}


def title(step):
    data = step["data"]
    kind = data["kind"]
    if kind == "model":
        return data["request"]["model"]["model"] + (" · вспомогательный запрос" if data["origin"] == "auxiliary" else " · основной запрос")
    if kind == "tool": return data["call"]["name"]
    if kind == "compaction": return "Сжатие истории"
    event = data["trace"]["input"]["event"]
    return event["event"]


def status(step):
    data = step["data"]
    kind = data["kind"]
    if kind == "model":
        response = data.get("response")
        if data.get("failure") or (response and response["finish_reason"] == "error"): return "Ошибка"
        if response is None: return "Нет результата"
    if kind == "tool":
        result = data.get("result")
        if result is None: return "Нет результата"
        if not result["ok"]: return "Ошибка"
    if kind == "hook" and any(item["outcome"]["status"] == "failed" for item in data["trace"]["steps"]): return "Ошибка"
    if kind == "compaction" and not data["report"]["changed"]: return "Без изменений"
    return "Готово"


class Analysis(Analytics):
    def __init__(self):
        super().__init__()
        self.turn = None
        self.step = None
        self.kind = "all"
        self.query = ""

    def render(self, view, surface, argument):
        if not self.selected_session:
            self.show(view,text("Нет выбранной сессии.","muted"))
            return
        data = self.fetch("/analysis", {"turn_id":self.turn} if self.turn else None)
        selected = data.get("selected")
        header = [text("Анализ ходов"),text("По сохранённым фактам. Просмотр не запускает ход и не меняет активный чат.","muted"),
                  stack(self.session_picker(),button("Обновить","refresh"),direction="row")]
        turns = [(turn["turn_id"],f"Ход {turn['number']} · {TURN_STATUS[turn['status']]} · {turn['prompt_preview'][:80]}") for turn in reversed(data["turns"])]
        header.append(select("turn",dict(turns).get((selected or {}).get("turn_id"),"Ходы"),turns,(selected or {}).get("turn_id"),"turn"))
        if not selected:
            self.show(view,stack(*header,text("В журнале пока нет ходов.","muted")))
            return
        steps = selected["steps"]
        if not any(step["sequence"] == self.step for step in steps):
            self.step = next((step["sequence"] for step in steps if status(step)=="Ошибка"), steps[0]["sequence"] if steps else None)
        models = [step["data"] for step in steps if step["data"]["kind"] == "model"]
        usages = [(model.get("response") or {}).get("usage") for model in models]
        header += [stack(metric("Запросов модели",len(models)),metric("Инструментов",sum(step["data"]["kind"]=="tool" for step in steps)),
                         metric("Ошибок",sum(status(step)=="Ошибка" for step in steps)),metric("Без результата",sum(status(step)=="Нет результата" for step in steps)),
                         metric("Вход / выход",f"{tokens(sum(usage['input_tokens'] for usage in usages if usage))} / {tokens(sum(usage['output_tokens'] for usage in usages if usage))}"),direction="row"),
                   select("kind",KINDS.get(self.kind,"Все шаги"),[("all","Все шаги")]+list(KINDS.items()),self.kind,"kind"),
                   {"type":"input","id":"query","value":self.query,"placeholder":"Найти шаг","action":"query"}]
        shown = [step for step in steps if (self.kind=="all" or self.kind==step["data"]["kind"]) and self.query.lower() in title(step).lower()]
        rows = []
        for step in shown:
            elapsed = "—" if step.get("finished_at_ms") is None else f"{max(0,step['finished_at_ms']-step['started_at_ms'])/1000:.2f} с"
            state = status(step)
            rows.append({"id":str(step["sequence"]),"label":title(step),"detail":f"{KINDS[step['data']['kind']]} · {state} · {elapsed}","selected":step["sequence"]==self.step,"tone":"danger" if state=="Ошибка" else "warning" if state=="Нет результата" else "default"})
        detail = next((step for step in steps if step["sequence"]==self.step),None)
        body = header + [{"type":"tree","id":"steps","rows":rows,"action":"step"}]
        if detail:
            record = detail["data"]
            kind = record["kind"]
            inputs = record.get("request") if kind=="model" else record.get("call") if kind=="tool" else record.get("trace",{}).get("input") if kind=="hook" else record
            outputs = {key:record[key] for key in ("response","failure","messages","result","resolution","approval_reason") if key in record}
            if kind=="hook": outputs=record["trace"]
            body.append({"type":"tabs","tabs":[{"label":"Вход","children":[code("step-input",inputs)]},
                {"label":"Результат","children":[code("step-output",outputs)]},
                {"label":"Полная запись","children":[code("step-raw",detail)]},
                {"label":"Ход и конфигурация","children":[code("turn-state",{key:value for key,value in selected.items() if key!="steps"})]}]})
        self.show(view,{"type":"fill","children":body})

    def action(self, view, action, value):
        super().action(view,action,value)
        if action in ("turn","kind","query"):
            setattr(self,action,value)
            if action=="turn": self.step=None
        elif action=="step": self.step=int(value["id"])
        elif action=="session": self.turn,self.step=None,None


if __name__ == "__main__":
    Analysis().run()
