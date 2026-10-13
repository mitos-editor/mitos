package sample

func scopes(count int, value any, ch chan int) {
    if count := 1; count > 0 {
        println(count)
//              ^ @variable
    } else {
        println(count)
//              ^ @variable
    }
    println(count)
//          ^ @variable.parameter

    for count := range 3 {
        println(count)
//              ^ @variable
    }
    println(count)
//          ^ @variable.parameter

    for count = range 3 {
        println(count)
//              ^ @variable.parameter
    }

    switch count := 1; count {
    case 1:
        println(count)
//              ^ @variable
    }
    println(count)
//          ^ @variable.parameter

    switch count := value.(type) {
    case int:
        println(count)
//              ^ @variable
    }
    println(count)
//          ^ @variable.parameter

    select {
    case count := <-ch:
        println(count)
//              ^ @variable
    default:
        println(count)
//              ^ @variable.parameter
    }
    println(count)
//          ^ @variable.parameter
}
