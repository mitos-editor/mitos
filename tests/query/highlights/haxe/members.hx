class C { function g(o) { o.meth(); var y = o.fld; freefn(); } }
//                          ^ @function.method.call
//                                            ^ @variable.other.member
//                                                 ^ @function.call
