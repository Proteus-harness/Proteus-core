import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Extension, stack, text


class Mode(Extension):
    def render(self, view, surface, argument):
        config = (self.state or {}).get("config")
        current = config["permission_mode"].lower() if config else "normal"
        if surface == "badge":
            self.badge(view, "Режим разрешений", ["Что агент делает без подтверждения."],
                       {"type": "text", "caret":True, "text": current, "tone": "muted"}, hidden=config is None)
        elif surface == "popup":
            self.show(view, stack(text("Режим разрешений", "muted"), *[
                {"type": "choice", "label": mode, "selected": mode == current,
                 "action": "mode", "value": mode} for mode in ("normal", "auto", "plan")]))

    def action(self, view, action, value):
        if action == "mode":
            self.set_session("mode", value)
            self.send({"type": "close_popup"})


if __name__ == "__main__":
    Mode().run()
