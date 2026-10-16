import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Extension, tokens


class Context(Extension):
    def render(self, view, surface, argument):
        usage = (self.state or {}).get("context")
        if not usage:
            self.badge(view, "Контекст", ["Появится после первого запроса к модели."])
            return
        used, maximum, trigger = usage["used"], usage["max"], usage["trigger"]
        percent = min(100, used / maximum * 100)
        lines = [f"Занято {tokens(used)} из {tokens(maximum)}"]
        tone = "muted"
        if trigger is not None:
            lines.append(f"Автокомпакт при {tokens(trigger)}")
            tone = "danger" if used >= trigger else "warning" if used >= trigger * .8 else "muted"
        self.badge(view, f"Контекст: {percent:.0f}%", lines,
                   {"type": "ring", "percent": percent, "tone": tone})


if __name__ == "__main__":
    Context().run()
