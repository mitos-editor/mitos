export fn main() void = {
//        ^ @function.definition
    io::print("hello");
//      ^ @function.call
    let enabled = true;
//                ^ @constant.builtin.boolean
};
