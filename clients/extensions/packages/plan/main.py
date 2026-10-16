import json
import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Extension


class Plan(Extension):
    def render(self, view, surface, argument):
        steps = []
        for entry in reversed(self.entries):
            tool = entry.get("tool")
            if tool and tool["name"] == "update_plan":
                args = tool.get("effective_args") or tool["args"]
                if isinstance(args, str):
                    args = json.loads(args)
                steps = args.get("plan", [])
                break
        if not steps:
            self.badge(view, "План", ["Агент ещё не составил план."])
            return
        done = sum(step["status"] == "completed" for step in steps)
        marks = {"completed": "✓", "in_progress": "•", "pending": "○"}
        self.badge(view, f"План: {done} из {len(steps)}",
                   [f"{marks.get(step['status'], '○')} {step['step']}" for step in steps],
                   {"type": "text", "icon":True, "text": f"{done}/{len(steps)}", "tone": "muted"})


if __name__ == "__main__":
    Plan().run()
