class C { void G(O o){ o.meth(); int y = o.fld; freefn(); } }
//                       ^ @function.method
//                                         ^ @variable.other.member
//                                              ^ @function
