// dsp_mux_tx.v
// TX DSP Router: Routes the selected input through a processing path.
// When enabled = 0 (bypass mode), routes dma_i0/q0 and dma_i1/q1 directly to dac_i0/q0
// and dac_i1/q1 (matches stock AD9361 dual-channel DMA passthrough).
// When enabled = 1, routes the selected input channel to the custom DSP processing path
// and routes the processed modulated output to the selected DAC channel, while holding
// the non-selected DAC channel at zero.

`timescale 1ns / 1ps

module dsp_mux_tx (
    input wire clk,
    input wire sel,
    input wire enabled,     // 0 = bypass (DMA to DAC direct), 1 = custom DSP active
    
    // Raw inputs
    input wire [15:0] dma_i0,
    input wire [15:0] dma_q0,
    input wire [15:0] dma_i1,
    input wire [15:0] dma_q1,
    
    // Input for the processing chain
    output reg [15:0] proc_i,
    output reg [15:0] proc_q,
    
    // Output from the processing chain
    input wire [15:0] modulated_i,
    input wire [15:0] modulated_q,
    
    // Outputs to DAC (axi_ad9361)
    output reg [15:0] dac_i0,
    output reg [15:0] dac_q0,
    output reg [15:0] dac_i1,
    output reg [15:0] dac_q1
);
    always @(posedge clk) begin
        // The processing path inputs always receive the selected DMA channel.
        // This avoids a 3-way multiplexer on proc_i and proc_q, saving logic and routing resources.
        proc_i <= sel ? dma_i1 : dma_i0;
        proc_q <= sel ? dma_q1 : dma_q0;

        if (!enabled) begin
            // Bypass mode: direct DMA to DAC passthrough (matches original ADI design)
            dac_i0 <= dma_i0;
            dac_q0 <= dma_q0;
            dac_i1 <= dma_i1;
            dac_q1 <= dma_q1;
        end else if (sel) begin
            dac_i0 <= 16'sd0;
            dac_q0 <= 16'sd0;
            dac_i1 <= modulated_i;
            dac_q1 <= modulated_q;
        end else begin
            dac_i0 <= modulated_i;
            dac_q0 <= modulated_q;
            dac_i1 <= 16'sd0;
            dac_q1 <= 16'sd0;
        end
    end
endmodule