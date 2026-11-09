struct Point { int x; };
//     ^ @type
//                 ^ @variable.other.member
int add(int a) {
//  ^ @function.definition
//          ^ @variable.parameter
    return helper(a);
//         ^ @function.call
}
