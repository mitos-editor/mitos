function App() {
//       ^ @constructor
  const name = greet("x");
//      ^ @variable
//             ^ @function.call
  return <div className="box"><Widget count={name} /></div>;
//        ^ @tag
//            ^ @attribute
//                             ^ @constructor
//                                    ^ @attribute
//                                           ^ @variable
}
