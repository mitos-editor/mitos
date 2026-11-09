%Pair = type { i32, i32 }
; ^ @type

define i32 @identity(i32 %value) {
;          ^ @function.definition
;                        ^ @variable.parameter
entry:
  %result = call i32 @helper(i32 %value)
;                    ^ @function.call
  ret i32 %result
}
