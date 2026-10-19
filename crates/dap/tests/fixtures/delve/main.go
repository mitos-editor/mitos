package main

//go:noinline
func add(a, b int) int {
	value := a + b
	return value // breakpoint
}

func main() {
	println(add(20, 22))
}
