export fn main() void = {
//        ^ @function
    io::print("hello");
//      ^ @function
    let enabled = true;
//                ^ @constant.builtin.boolean
};
