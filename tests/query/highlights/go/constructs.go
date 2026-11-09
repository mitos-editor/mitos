package main
//      ^ @namespace
type Point struct { X int }
//   ^ @type.struct.definition
//                  ^ @variable.other.member
//                    ^ @type.builtin
func (p Point) Dist(count int) bool {
//             ^ @function.method.public.definition
//                  ^ @variable.parameter
    return helper(count)
//         ^ @function.call
}
