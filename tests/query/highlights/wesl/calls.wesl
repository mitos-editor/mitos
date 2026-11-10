fn run() {
// ^ @function.definition
    let y = bar(2);
//          ^ @function.call
    let z = obj.field;
//              ^ @variable.other.member
}
