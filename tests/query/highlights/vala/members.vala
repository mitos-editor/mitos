void g(Obj o) { o.meth(); var a = o.fld; var b = Foo.BAR; }
//                ^ @function.call
//                                  ^ @variable.other.member
//                                                   ^ @constant
