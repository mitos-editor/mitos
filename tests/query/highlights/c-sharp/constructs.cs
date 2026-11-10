class Box {
//    ^ @type
    void M() {
//       ^ @function.method.definition
        Helper(1);
//      ^ @function.call
        obj.Do();
//          ^ @function.method.call
    }
}
