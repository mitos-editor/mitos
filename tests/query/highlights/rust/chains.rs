fn m() {
    let r = a.b.c().d.e();
//            ^ @variable.other.member
//              ^ @function.method.call
//                  ^ @variable.other.member
//                    ^ @function.method.call
}
