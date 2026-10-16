import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Extension


class Session(Extension):
    def render(self, view, surface, argument):
        state = self.state
        if state is None:
            self.badge(view, "Сессия", ["Сессия не открыта."])
            return
        activity = (state.get("activity") or {}).get("status")
        status = {"running": "Работает", "waiting_approval": "Ждёт подтверждения",
                  "waiting_input": "Ждёт ответа"}.get(activity, "Ожидает")
        if not state["connected"]:
            status = "Нет связи"
        elif state["execution"]["active"] and status == "Ожидает":
            status = "Работает"
        messages = sum(entry["role"] in ("user", "assistant") and not entry.get("tool")
                       and not entry.get("subagent") for entry in self.entries)
        tools = sum(bool(entry.get("tool")) + len((entry.get("subagent") or {}).get("tools", []))
                    for entry in self.entries)
        pending = state.get("pending") or {}
        waiting = len(pending.get("approvals", [])) + len(pending.get("user_inputs", []))
        queued = len(pending.get("queued_user_messages", []))
        tone = "warning" if not state["connected"] or activity in ("waiting_approval", "waiting_input") else "muted"
        self.badge(view, f"Сессия: {status.lower()}",
                   [f"Сообщений: {messages}", f"Вызовов инструментов: {tools}",
                    f"Ожидают ответа: {waiting}", f"В очереди: {queued}"], {"type": "icon", "tone": tone})


if __name__ == "__main__":
    Session().run()
