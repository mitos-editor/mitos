let _ = helper x
(*      ^ @function.call *)
let _ = Mod.doThing 1
(*      ^ @namespace *)
(*          ^ @function.call *)
