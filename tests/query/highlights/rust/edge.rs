fn m() {
    obj.first().second();
//      ^ @function.method.call
//               ^ @function.method.call
    let n = outer(inner(x));
//          ^ @function.call
//                ^ @function.call
    let f = build().field;
//                  ^ @variable.other.member
}
