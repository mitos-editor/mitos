class Box {
//    ^ @type.class.definition
    boolean method(int count) {
//  ^ @type.builtin
//          ^ @function.method.definition
        String s = "a\tb";
//      ^ @type
//                   ^ @constant.character.escape
        return helper(count);
//             ^ @function.method.call
    }
}
