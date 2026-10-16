import sys
from pathlib import Path
sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk"))
from proteus_extension import Extension


class Terminal(Extension):
    def render(self, view, surface, argument):
        self.show(view, {"type":"fill", "children":[{"type":"terminal", "id":"shell", "cwd":self.workspace}]})


if __name__ == "__main__":
    Terminal().run()
