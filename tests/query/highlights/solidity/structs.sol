contract Example {
    struct Point {
//         ^^^^^ @type
        uint value;
//           ^^^^^ @variable.other.member
    }

    function make(uint input) public pure returns (Point memory) {
        Point{value: input};
//      ^^^^^ @type
//            ^^^^^ @variable.other.member
//                   ^^^^^ @variable.parameter
    }
}
