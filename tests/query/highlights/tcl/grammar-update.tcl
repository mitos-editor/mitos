regsub -all {foo} $input {bar}
# ^ @function.builtin
#            ^ @string
#                 ^ @variable
#                         ^ @string
regsub {$name} $input {[expr 1]}
#         ^ @string
#                      ^ @string
puts {*}
#     ^ @string
puts {*}$args
#    ^ @operator
#        ^ @variable
