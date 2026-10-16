import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Extension, stack, text, button


class Model(Extension):
    def __init__(self):
        super().__init__()
        self.models_open = False

    def render(self, view, surface, argument):
        config = (self.state or {}).get("config") or {}
        model = config.get("model") or {}
        name = model.get("name", "model")
        options = config.get("model_options", [])
        models = [option for option in options if not option["hidden"] or option["name"] == name]
        providers = {option["provider"] for option in options}
        reasoning = config.get("reasoning") or {}
        efforts = reasoning.get("effort_options", [])
        current = reasoning.get("effort") or ("none" if reasoning.get("enabled") is False else None)
        default = next((option.get("default_reasoning_effort") for option in options if option["name"] == name), None)
        pending = next((value for kind,value in reversed(list(self.pending.values())) if kind=="effort"), "unset")
        if pending != "unset": current = pending
        selected = (current or default) if (current or default) in efforts else (efforts[0] if efforts else None)
        if surface == "badge":
            label = name + (f" · {current}" if current is not None and len(efforts) > 1 else "")
            self.badge(view, "Модель и рассуждения", [f"Профиль {config.get('profile', '')} · {model.get('provider', '')}"],
                       {"type": "text", "caret":True, "text": label, "tone": "muted"},
                       hidden=not models and len(efforts) < 2)
        elif surface == "popup":
            if self.models_open or len(efforts) < 2:
                heading = "Модель" + (" · " + next(iter(providers)) if len(providers) == 1 else "")
                rows = [text(heading, "muted")]
                if self.models_open and len(efforts) > 1:
                    rows.insert(0, button("‹ Назад", "back"))
                rows += [{"type": "choice", "label": option["name"] if len(providers) == 1 else option["provider"] + "/" + option["name"],
                          "selected": option["name"] == name, "action": "model", "value": option["name"]} for option in models]
                self.show(view, stack(*rows))
            else:
                self.show(view, stack(
                    button(name + " ›", "models") if models else text(name),
                    {"type": "divider"},
                    stack(text("Рассуждения", "muted"), button("Как в профиле", "reset"), direction="row"),
                    {"type": "steps", "id": "reasoning", "options": efforts,
                     "selected": efforts.index(selected), "action": "effort"}))

    def event(self, event):
        if event == "session_opened":
            self.models_open = False

    def action(self, view, action, value):
        if action == "escape":
            if self.models_open: self.models_open=False
            else: self.send({"type":"close_popup"})
        elif action == "models":
            self.models_open = True
        elif action == "back":
            self.models_open = False
        elif action == "model":
            self.set_session("model", value)
            self.models_open = False
        elif action == "effort":
            config = (self.state or {}).get("config") or {}
            efforts = config.get("reasoning", {}).get("effort_options", [])
            if isinstance(value, int) and 0 <= value < len(efforts):
                self.set_session("effort", efforts[value])
        elif action == "reset":
            self.set_session("effort", None)


if __name__ == "__main__":
    Model().run()
