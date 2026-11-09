function f(x)
    obj.doThing(x)
#       ^ @function.call
    helper(x)
#   ^ @function.call
    y = obj.field
#           ^ @variable.other.member
end
