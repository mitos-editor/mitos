interface Paint {
//        ^ @type.interface.definition
    paint(): void;
//  ^ @function.method.declaration
}
class Widget {
//    ^ @type.class.definition
    paint() {}
//  ^ @function.method.definition
}
    widget.paint();
//         ^ @function.method.call
type Name = string;
//   ^ @type.alias.definition
