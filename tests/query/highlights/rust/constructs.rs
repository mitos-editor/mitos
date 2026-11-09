use std::fmt;
//  ^ @namespace
enum Color { Red }
//           ^ @type.enum.variant
fn f(self) {
//   ^ @variable.builtin
    println!("ok");
//  ^ @function.macro.call
    self.helper();
//       ^ @function.method.call
    let p = Point {};
//          ^ @constructor
}
