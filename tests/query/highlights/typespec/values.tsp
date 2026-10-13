import "lib.tsp";
//      ^ @string.special.path
const answer = 42;
//    ^ @constant
@doc("hello")
// ^ @attribute
model Widget { name: string; }
//    ^ @type
op read(): Widget;
// ^ @function
