contract C {
  function f() public {
    obj.doThing();
//      ^ @function.method.call
    helper(1);
//  ^ @function.call
    uint x = obj.field;
//               ^ @variable.other.member
  }
}
