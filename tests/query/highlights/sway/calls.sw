fn main() {
    obj.doThing();
//      ^ @function.method.call
    let x = obj.field;
//              ^ @variable.other.member
    helper(1);
//  ^ @function.call
}
