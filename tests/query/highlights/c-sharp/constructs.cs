class Box {
//    ^ @type
    void M() {
//       ^ @function.method
        Helper(1);
//      ^ @function
        obj.Do();
//          ^ @function.method
    }
}
