import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Analytics, stack, text, button, select, metric, code, tokens
import report


class Usage(Analytics):
    def __init__(self):
        super().__init__()
        self.tab = "requests"
        self.period = "all"
        self.model = None
        self.status = "all"
        self.order = "oldest"
        self.query = ""
        self.page = 0
        self.expanded = set()

    def render(self, view, surface, argument):
        if not self.selected_session:
            self.show(view, text("Нет выбранной сессии.", "muted"))
            return
        header = [text("Расход и контекст"), stack(self.session_picker(), button("Обновить", "refresh"), direction="row"),
                  stack(button("Запросы и расход", "tab", "requests"), button("Контекст", "tab", "context"), direction="row")]
        if self.tab == "context":
            snapshot = self.fetch("/context")
            used, maximum, cached, usage = report.context(snapshot)
            actual = usage.get("actual") or {}
            categories = usage.get("categories", [])
            body = [stack(metric("Занято", tokens(used)), metric("Размер окна", tokens(maximum) if maximum else "—"),
                          metric("Кэш", f"{cached / used * 100:.0f}%" if used and cached is not None else "—"), direction="row"),
                    text("Измерение провайдера" if usage.get("actual") else "Локальная оценка", "muted")]
            body.append({"type":"cells", "cells":report.cells(snapshot)})
            if maximum:
                body.append({"type":"progress", "value":used, "max":maximum, "label":"Окно контекста"})
            body += [{"type":"bar_chart", "bars":[{"label":item["name"], "value":item["tokens"]} for item in categories]},
                     {"type":"table", "columns":["Кэш","Токенов"], "rows":[["Прочитано",str(actual.get("cached_input_tokens") if actual.get("cached_input_tokens") is not None else "—")], ["Записано",str(actual.get("cache_creation_input_tokens") if actual.get("cache_creation_input_tokens") is not None else "—")]]},
                     code("context", snapshot, 360)]
            self.show(view, stack(*(header + body)))
            return
        snapshot = self.fetch("/usage")
        if snapshot is None:
            self.show(view, stack(*header, text("Журнал расхода недоступен для этого чата.", "muted")))
            return
        requests = report.scoped(snapshot, self.period, self.model)
        summary = report.totals(requests)
        rows = report.filtered(requests, self.status, self.query, self.order)
        models = list(dict.fromkeys(report.model_key(request) for request in snapshot["requests"]))
        turns = list(dict.fromkeys(request["turn_id"] for request in snapshot["requests"] if request.get("turn_id")))
        periods = [("all","Вся сессия"),("latest","Последний ход")] + [(turn,f"Ход {index+1}") for index,turn in enumerate(turns)]
        controls = stack(select("period",dict(periods).get(self.period,"Период"),periods,self.period,"period"),
                         select("model",self.model or "Все модели",[(None,"Все модели")]+[(model,model) for model in models],self.model,"model"),
                         select("status",{"all":"Все запросы","problems":"С проблемами","missing":"Без usage"}[self.status],[("all","Все запросы"),("problems","С проблемами"),("missing","Без usage")],self.status,"status"),
                         select("order",self.order,[("oldest","По порядку"),("newest","Сначала последние"),("longest","Сначала долгие"),("tokens","Больше токенов"),("cost","Сначала дороже")],self.order,"order"),direction="row")
        body = [controls, stack(metric("Запросов",summary["requests"],f"Usage: {summary['reported']} · auxiliary: {summary['auxiliary']}"),
                 metric("Вход",tokens(summary["input_tokens"])),metric("Выход",tokens(summary["output_tokens"])),
                 metric("Стоимость",f"${summary['cost']:.5f}" if summary["priced"] else "—",f"Тариф известен для {summary['priced']} из {summary['requests']} запросов"),direction="row"),
                stack(metric("Кэш прочитан",tokens(summary["cached_input_tokens"])),metric("Кэш записан",tokens(summary["cache_creation_input_tokens"])),
                      metric("Рассуждения",tokens(summary["reasoning_output_tokens"])),metric("Ошибки / без завершения",f"{summary['errors']} / {summary['unfinished']}"),direction="row"),
                text(f"Оценка API-эквивалента · таблица {report.PRICE_DATE} · {report.PRICE_SOURCE}. Рассуждения входят в output; без usage расход неизвестен.","muted"),
                {"type":"input","id":"query","value":self.query,"placeholder":"ID запроса, хода или модель","action":"query"}]
        by_model = [(model,report.totals([request for request in requests if report.model_key(request)==model])) for model in models]
        body.append({"type":"table","columns":["Модель","Запросов","Вход","Выход","Стоимость"],"rows":[[model,str(total["requests"]),tokens(total["input_tokens"]),tokens(total["output_tokens"]),f"${total['cost']:.5f}" if total["priced"] else "—"] for model,total in by_model if total["requests"]]})
        pages = max(1,(len(rows)+19)//20)
        self.page = min(self.page,pages-1)
        for request in rows[self.page*20:(self.page+1)*20]:
            usage = request.get("usage")
            price = report.cost(request)
            label = f"{report.model_key(request)} · {request['status']} · {report.duration(request)/1000:.2f} с · "
            label += (f"{usage['input_tokens']} → {usage['output_tokens']} токенов" if usage else "usage нет")
            label += f" · ${price:.5f}" if price is not None else " · тариф неизвестен"
            body.append(button(label,"request",request["exchange_id"]))
            if request["exchange_id"] in self.expanded:
                body.append(code(request["exchange_id"],request))
        body.append(stack(button("‹","page",max(0,self.page-1)),text(f"{self.page+1} / {pages} · {len(rows)} запросов"),button("›","page",min(pages-1,self.page+1)),direction="row"))
        self.show(view, stack(*(header+body)))

    def action(self, view, action, value):
        super().action(view,action,value)
        if action in ("tab","period","model","status","order","query","page"):
            setattr(self,action,value)
            if action != "page": self.page=0
        elif action == "request":
            self.expanded.symmetric_difference_update({value})
        elif action == "session":
            self.period,self.model,self.page="all",None,0
            self.expanded.clear()


if __name__ == "__main__":
    Usage().run()
