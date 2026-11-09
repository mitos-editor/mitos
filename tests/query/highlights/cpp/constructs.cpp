class Widget {
//    ^ @type
    int field;
//      ^ @variable.other.member
    bool method(int count) {
//       ^ @function.method.definition
//                  ^ @variable.parameter
        return helper(count);
//             ^ @function.call
    }
};
