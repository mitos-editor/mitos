let helper = fun value => value in
#                ^ @variable.parameter
{ result = helper 1, field = { nested = 2 }.nested }
# ^ @variable.other.member
#          ^ @function
