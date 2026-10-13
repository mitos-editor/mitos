%Pair = type { i32, i32 }
; ^ @type

define i32 @identity(i32 %value) {
;          ^ @function
;                        ^ @variable.parameter
entry:
  %result = call i32 @helper(i32 %value)
;                    ^ @function
  ret i32 %result
}
