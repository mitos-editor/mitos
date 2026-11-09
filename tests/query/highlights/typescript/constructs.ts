interface Shape { size: number; }
//        ^ @type.interface.definition
//                ^ @variable.other.member
//                      ^ @type.builtin
class Box {
//    ^ @type.class.definition
  method(count: number): void {
//^ @function.method.definition
    obj.doThing();
//      ^ @function.method.call
  }
}
