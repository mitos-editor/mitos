@Deprecated("old")
//^ @attribute
fun greet(name: String) = name.trim()
//        ^ @variable.parameter
//                             ^ @function.method

val action = { value: Int -> value }
//             ^ @variable.parameter
