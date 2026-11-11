import pytest
from go_review import review


def test_sibling_positions_are_isolated():
    tree = "(;SZ[3]AB[aa](;AW[cc])(;AE[aa]AW[ba]))"
    samples = [([0], ["B..","...","..W"], (0,0,7)), ([1], [".W.","...","..."], (0,8,0)),
               ([], ["B..","...","..."], (8,0,0)), ([0], ["B..","...","..W"], (0,0,7))]
    for path, board, (black, white, neutral) in samples:
        assert review(tree, path) == {"board": board, "regions": {"black": black, "white": white, "neutral": neutral}}


def test_surrounded_white_region_and_column_order():
    tree = "(;SZ[3]AB[ba][ab][cb][bc];AE[ba][cb][ab][bc]AW[ca])"
    assert review(tree, [])["regions"] == {"black":5, "white":0, "neutral":0}
    assert review(tree, [0]) == {"board": ["..W","...","..."], "regions": {"black":0,"white":8,"neutral":0}}


@pytest.mark.parametrize("tree,path", [
    ("(;SZ[1])",[0]), ("(;SZ[2];AW[aa])",[-1]), ("(;SZ[2];AW[aa])",[False]),
    ("(;SZ[2];AW[aa])",[0.0]), ("(;SZ[0])",[]), ("(;SZ[20])",[]), ("(;SZ[x])",[]),
    ("(;C[none])",[]), ("(;SZ[2]AB[cc])",[]), ("(;SZ[2]AW[Aa])",[]),
    ("(;SZ[2]AE[a])",[]), ("(;SZ[2]AW[abc])",[])])
def test_invalid(tree,path):
    with pytest.raises(ValueError):
        review(tree, path)
