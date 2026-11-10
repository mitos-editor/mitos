def g(o) { o.meth(); def y = o.fld; def z = o.CONST; freefn() }
//           ^ @function.call
//                             ^ @variable.other.member
//                                            ^ @constant
//                                                   ^ @function.call
