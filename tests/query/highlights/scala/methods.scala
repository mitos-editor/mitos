object O {
  def m() = {
    invokeit()
//  ^ @function.call
    obj.doThing()
//      ^ @function.method.call
    val x = obj.field
//              ^ @variable.other.member
  }
}
