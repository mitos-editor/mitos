package main

func main() {
	invokeit()
//	^ @function

    (invokeit)()
//   ^ @function
    (obj.Method)()
//       ^ @function.method
    (len)(items)
//   ^ @function.builtin
    callbacks[0]()
//  ^ @variable
}
