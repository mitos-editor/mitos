fn main() {
    module::execute();
//          ^ @function
    object.render();
//         ^ @function.method
    let value = Option::Some(1);
//                      ^ @constructor
}
