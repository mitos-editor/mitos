const std = @import("std");
//          ^ @keyword.control.import
pub fn main() void {
    obj.doThing();
//      ^ @function.method.call
    helper(x);
//  ^ @function.call
    const y = obj.field;
//                ^ @variable.other.member
}
