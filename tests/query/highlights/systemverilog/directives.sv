`nounconnected_drive
//^ @keyword.directive

`define WIDTH 8
//^ @keyword.directive

module example;
  initial $display(`WIDTH);
//                  ^ @function.macro
endmodule
