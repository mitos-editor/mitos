package roles
type Widget struct {}
//   ^ @type.struct.definition
type Paint interface {
    Paint()
//  ^ @function.method.public.declaration
}
func External()
//   ^ @function.public.declaration
func Draw() {
//   ^ @function.public.definition
    Draw()
//  ^ @function.call
}
func (w Widget) Paint() {
//              ^ @function.method.public.definition
    w.Paint()
//    ^ @function.method.call
}
