def add(a, b):
    return a + b

def multiply(a, b):
    return a * b

def subtract(a, b):
    return a - b

def divide(a, b):
    if b == 0:
        return 0
    return a // b

def power(base, exp):
    if exp == 0:
        return 1
    result = 1
    for _ in range(exp):
        result *= base
    return result