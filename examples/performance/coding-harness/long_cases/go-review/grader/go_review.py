from go_counting import Board
from sgf_parsing import parse


def review(sgf, path):
    node = parse(sgf)
    try:
        size_text, = node.properties["SZ"]
        size = int(size_text)
        if not 1 <= size <= 19:
            raise ValueError("size")
    except (KeyError, ValueError):
        raise ValueError("invalid size") from None
    cells = [[" "] * size for _ in range(size)]
    for i in range(len(path) + 1):
        for key, value in [("AE", " "), ("AB", "B"), ("AW", "W")]:
            for point in node.properties.get(key, []):
                if len(point) != 2:
                    raise ValueError("invalid point")
                x, y = (ord(c) - ord("a") for c in point)
                if not (0 <= x < size and 0 <= y < size):
                    raise ValueError("invalid point")
                cells[y][x] = value
        if i < len(path):
            idx = path[i]
            if type(idx) is not int or not 0 <= idx < len(node.children):
                raise ValueError("invalid path")
            node = node.children[idx]
    board = ["".join(row) for row in cells]
    territory = Board(board).territories()
    return {"board": [row.replace(" ", ".") for row in board],
            "regions": {name: len(territory[c]) for c, name in [("B","black"),("W","white"),("","neutral")]}}
