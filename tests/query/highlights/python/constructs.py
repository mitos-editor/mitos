import os
#      ^ @namespace
class Widget:
#     ^ @type.class.definition
    def method(self, count: int) -> bool:
#       ^ @function.method.definition
#              ^ @variable.builtin
#                    ^ @variable.parameter
#                           ^ @type.builtin
        return self.helper(count)
#                   ^ @function.method.call
