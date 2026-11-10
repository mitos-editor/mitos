class Widget {
//    ^ @type.class.definition
    paint() {}
//  ^ @function.method.definition
    #hidden() {}
//  ^ @function.method.private.definition
    draw() { this.#hidden(); }
//                ^ @function.method.private.call
}
function draw() {}
//       ^ @function.definition
    draw();
//  ^ @function.call
    widget.paint();
//         ^ @function.method.call
    widget.color;
//         ^ @variable.other.member
