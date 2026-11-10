package main

func main() {
	invokeit()
//	^ @function.call

    (invokeit)()
//   ^ @function.call
    (obj.Method)()
//       ^ @function.method.call
    (len)(items)
//   ^ @function.builtin.call
    callbacks[0]()
//  ^ @variable
}
