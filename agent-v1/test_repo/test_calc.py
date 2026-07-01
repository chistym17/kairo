from calc import add, multiply

def test_add():
    assert add(2, 3) == 5
    assert add(15, 4) == 19

def test_multiply():
    assert multiply(3, 4) == 12
