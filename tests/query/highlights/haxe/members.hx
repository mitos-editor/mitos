class C { function g(o) { o.meth(); var y = o.fld; freefn(); } }
//                          ^ @function.method
//                                            ^ @variable.other.member
//                                                 ^ @function
