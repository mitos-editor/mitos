trait Greeter:
#     ^ @type
    def greet(self):
#             ^ @variable.builtin
        print("hi")
#       ^ @function.builtin.call

def use_value(read value: Int):
#                  ^ @variable.parameter
    pass
