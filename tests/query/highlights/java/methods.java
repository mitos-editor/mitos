class C {
    int m() {
        invokeit();
//      ^ @function.method.call
        obj.doThing();
//          ^ @function.method.call
        return 0;
    }
}
