fn main() {
    module::execute();
//          ^ @function.call
    object.render();
//         ^ @function.method.call
    let value = Option::Some(1);
//                      ^ @constructor
}
