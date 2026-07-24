// tx_strobe_gen.v
// Generates a programmable clock-enable strobe on the FPGA fabric clock
// (axi_ad9361 l_clk) using a 16-bit phase accumulator.
//
// Operation:
//   Strobe rate = l_clk * phase_inc / 65536.
//   The strobe enable output is asserted on the overflow bit of the phase accumulator.
//   When phase_inc is 0 (register default at boot), the accumulator does not increment
//   and the strobe remains low.

`timescale 1ns / 1ps

module tx_strobe_gen (
    input  wire clk,
    (* X_INTERFACE_IGNORE = "TRUE" *)
    input  wire [15:0] phase_inc, // 16-bit programmable phase increment
    (* X_INTERFACE_IGNORE = "TRUE" *)
    output wire strobe_out
);
    // Phase Accumulator for Programmable Mode
    reg [16:0] accumulator = 17'b0;
    always @(posedge clk) begin
        accumulator <= accumulator[15:0] + phase_inc;
    end
    assign strobe_out = accumulator[16];
endmodule
