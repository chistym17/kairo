def add(a, b):
    return a + b

def subtract(a, b):
    return a - b

def multiply(a, b):
    return a * b

def divide(a, b):
    return a / b

# Test the add function
def test_add():
    assert add(2, 3) == 5

# Test the subtract function
def test_subtract():
    assert subtract(2, 3) == -1

# Test the multiply function
def test_multiply():
    assert multiply(2, 3) == 6

# Test the divide function
def test_divide():
    assert divide(2, 3) == 0.6666666666666666
