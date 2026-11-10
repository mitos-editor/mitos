fn f(o Obj) {
  o.do_thing()
//  ^ @function.method.call
  y := o.field
//       ^ @variable.other.member
}
