package main
func Sum[T Number](xs []T) T {
//       ^ @type.parameter
//         ^ @type
//                      ^ @type.parameter
//                         ^ @type.parameter
	var f = obj.Method
//           ^ @variable.other.member
	return obj.field
//          ^ @variable.other.member
}

type Box[T any] struct { Value T }
//   ^ @type.definition
//                             ^ @type.parameter

type Values[T any] = []T
//                     ^ @type.parameter

func (box Box[T]) Get() T {
//            ^ @type.parameter
//                      ^ @type.parameter
    return box.Value
}

func (box *Box[T]) Set(value T) {
//             ^ @type.parameter
//                           ^ @type.parameter
    box.Value = value
}

type (
    First[T any] struct { Value T }
//                              ^ @type.parameter
    Second[T any] struct { Value T }
//                               ^ @type.parameter
    Third struct { Value T }
//                       ^ @type
)

func instantiate(value any) {
    pkg.Map[int, string](1, "one")
//      ^ @function.method
    Box[int](value)
//  ^ @type
    _ = Box[int]{Value: 1}
//      ^ @type
}
