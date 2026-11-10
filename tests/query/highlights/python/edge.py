def h():
    m = obj.method
#           ^ @variable.other.member
    s = f"{obj.compute()}"
#              ^ @function.method.call
    obj.first().second()
#       ^ @function.method.call
#                ^ @function.method.call
