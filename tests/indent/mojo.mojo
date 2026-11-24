trait Greeter:
    def greet(self):
        pass

def main():
    if True:
        print("hello")

def require_value(has_value: Bool):
    __guard has_value else:
        return
    print("value")
