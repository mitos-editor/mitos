trait Greeter:
#     ^ @type
    def greet(self):
#             ^ @variable.builtin
        print("hi")
#       ^ @function.builtin.call

def use_value(read value: Int):
#                  ^ @variable.parameter
    pass

def require_value(has_value: Bool):
    __guard has_value else:
#   ^ @keyword.control.conditional
        return
