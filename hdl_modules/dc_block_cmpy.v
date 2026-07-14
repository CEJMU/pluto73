// dc_block_cmpy.v
// DC blocking filter placed directly after the complex multiplier (rx_cmpy).
// Takes the 64-bit packed cmpy output [I(31:0), Q(63:32)], applies a
// first-order IIR high-pass to each channel, outputs separate 32-bit I and Q.
// Replaces rx_cmpy_out_i and rx_cmpy_out_q slices.
//
// H(z) = (1 - z^-1) / (1 - alpha*z^-1),  alpha = 255/256
// Uses arithmetic shift (>>>) -- no multipliers, no DSP slices.
// 40-bit accumulator for headroom on the 32-bit input.

`timescale 1ns / 1ps

module dc_block_cmpy (
    input wire clk,
    input wire rst_n,

    // 64-bit packed input from rx_cmpy (I = bits 31:0, Q = bits 63:32)
    input wire [63:0] din,

    // DC-blocked outputs (32-bit each, matching CIC input width)
    output wire signed [31:0] out_i,
    output wire signed [31:0] out_q
);

    wire signed [31:0] in_i = din[31:0];
    wire signed [31:0] in_q = din[63:32];

    // 32-bit raw sample registers; sign-extended to 40 bits on use.
    // (the top 8 bits are redundant with bit 31 and don't need their own FFs)
    reg signed [31:0] prev_i, prev_q;
    reg signed [39:0] acc_i, acc_q;

    // y[n] = x[n] - x[n-1] + alpha*y[n-1]
    // alpha = 255/256, so alpha*y = y - (y >>> 8)
    wire signed [39:0] x_i = {{8{in_i[31]}}, in_i};
    wire signed [39:0] x_q = {{8{in_q[31]}}, in_q};
    wire signed [39:0] prev_x_i = {{8{prev_i[31]}}, prev_i};
    wire signed [39:0] prev_x_q = {{8{prev_q[31]}}, prev_q};

    wire signed [39:0] next_i = x_i - prev_x_i + acc_i - (acc_i >>> 8);
    wire signed [39:0] next_q = x_q - prev_x_q + acc_q - (acc_q >>> 8);

    // Saturate 40-bit accumulator to 32-bit output.
    assign out_i = (acc_i > 40'sh7FFFFFFF)  ?  32'sh7FFFFFFF :
                   (acc_i < -40'sh80000000) ? -32'sh80000000 :
                   acc_i[31:0];

    assign out_q = (acc_q > 40'sh7FFFFFFF)  ?  32'sh7FFFFFFF :
                   (acc_q < -40'sh80000000) ? -32'sh80000000 :
                   acc_q[31:0];

    always @(posedge clk) begin
        if (!rst_n) begin
            acc_i  <= 0;
            acc_q  <= 0;
        end else begin
            acc_i  <= next_i;
            acc_q  <= next_q;
            prev_i <= x_i[31:0];
            prev_q <= x_q[31:0];
        end
    end
endmodule
