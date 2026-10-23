class_name MyNode
#          ^ @type
extends Node
#       ^ @type.builtin
@export var speed: float = 1.0
#^ @attribute
#           ^ @variable.other.member
#                  ^ @type.builtin
#                          ^ @constant.numeric.float
func move(delta: float) -> void:
#    ^ @function
#         ^ @variable.parameter
#                          ^ @type.builtin
    print(Vector2.ZERO)
#   ^ @function.builtin
#         ^ @type.builtin
#                 ^ @constant
