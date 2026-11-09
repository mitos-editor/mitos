class C { void G(O o){ o.meth(); int y = o.fld; freefn(); } }
//                       ^ @function.method.call
//                                         ^ @variable.other.member
//                                              ^ @function.call
