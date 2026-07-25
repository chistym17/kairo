from calc import add, multiply, subtract, divide, power

def test_add():
    assert add(2, 3) == 5
    assert add(-2, 3) == 1

def test_multiply():
    assert multiply(3, 4) == 12

def test_subtract():
    assert subtract(10, 4) == 6

def test_divide():
    assert divide(10, 3) == 3
    assert divide(7, 2) == 3
    assert divide(10, 0) == 0

def test_power():
    assert power(2, 3) == 8
    assert power(5, 0) == 1
    assert power(3, 2) == 9
