class Widget {
//    ^ @type
    int field;
//      ^ @variable.other.member
    bool method(int count) {
//       ^ @function.method
//                  ^ @variable.parameter
        return helper(count);
//             ^ @function
    }
};
