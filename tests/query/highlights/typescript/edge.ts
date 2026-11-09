const m = obj.method;
//            ^ @variable.other.member
const s = `${obj.compute()}`;
//               ^ @function.method.call
obj.first().second();
//  ^ @function.method.call
//           ^ @function.method.call
