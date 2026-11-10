interface Paint {
//        ^ @type.interface.definition
    void paint();
//       ^ @function.method.declaration
}
class Widget {
//    ^ @type.class.definition
    Widget() {}
//  ^ @constructor.definition
    void paint() {
//       ^ @function.method.definition
        this.paint();
//           ^ @function.method.call
}
}
