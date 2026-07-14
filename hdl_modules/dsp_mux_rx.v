// dsp_mux_rx.v
// RX DSP Multiplexer: Routes either in0 or in1 to the output, dependent on sel
// Used for routing data form the selected antenna (RX1 or RX2) into the audio proccessing path

`timescale 1ns / 1ps

module dsp_mux_rx (
    input wire clk,
    input wire sel,

    // Input
    input wire [15:0] in0_i,
    input wire [15:0] in0_q,
    input wire [15:0] in1_i,
    input wire [15:0] in1_q,

    // Output
    output reg [15:0] out_i,
    output reg [15:0] out_q
);
    always @(posedge clk) begin
        if (sel) begin
            out_i <= in1_i;
            out_q <= in1_q;
        end else begin
            out_i <= in0_i;
            out_q <= in0_q;
        end
    end
endmodule