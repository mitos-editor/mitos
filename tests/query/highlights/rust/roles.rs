struct Widget;
//     ^ @type.struct.definition
impl Widget {
    pub fn paint(&self) {}
//         ^ @function.method.public.definition
}
trait Paint {
    fn paint(&self);
//     ^ @function.method.declaration
}
fn draw() {
// ^ @function.definition
    widget.paint();
//         ^ @function.method.call
    consume(module::draw);
//                  ^ @function.reference
    let callback = draw::<u8>;
//                 ^ @function.reference
    draw::<u8>();
//  ^ @function.call
}
