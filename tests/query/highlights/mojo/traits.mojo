trait Greeter:
#     ^ @type
    def greet(self):
#             ^ @variable.builtin
        print("hi")
#       ^ @function.builtin

def use_value(read value: Int):
#                  ^ @variable.parameter
    pass
