void render(int &value) {
//               ^ @variable.parameter
  helper(value);
//^ @function.call
  Thing(value);
//^ @constructor.call
}
