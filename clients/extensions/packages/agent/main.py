import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Extension


class Agent(Extension):
    def render(self, view, surface, argument):
        config = (self.state or {}).get("config")
        if config is None:
            self.badge(view, "Агент", ["Загрузка…"])
            return
        model = config.get("model") or {}
        lines = [f"Модель: {model.get('label') or model.get('name', '—')}",
                 f"Режим: {config['permission_mode']}",
                 f"Инструментов: {len(config['registered_tools'])}"]
        lines += [f"{module['slot']}: {module['id']}" for module in config["modules"]]
        self.badge(view, f"Профиль {config['profile']}", lines)


if __name__ == "__main__":
    Agent().run()
