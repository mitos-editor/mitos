void render(int &value) {
//               ^ @variable.parameter
  helper(value);
//^ @function
  Thing(value);
//^ @constructor
}
