// dsp_mux_tx.v
// TX DSP Router & Dynamic Pacing Controller:
// Routes selected input through processing path and provides dynamic pacing.
//
// When enabled = 0 (bypass mode):
//   - Routes dma_i0/q0 and dma_i1/q1 directly to dac_i0/q0 and dac_i1/q1
//   - Drives fifo_rd_en with dac_valid for DAC-rate reading
//   - Holds proc_valid low
//
// When enabled = 1 (custom DSP active):
//   - Routes selected DMA channel to proc_i/proc_q combinationally
//   - Drives fifo_rd_en with dynamic feed_strobe (strobe_in divided by 4 * cic_interp)
//   - Drives proc_valid (tx_fir/s_axis_data_tvalid) with dma_valid (fifo_rd_valid)
//   - Routes modulated_i/q to selected DAC channel

`timescale 1ns / 1ps

module dsp_mux_tx (
    input wire clk,
    input wire sel,
    input wire enabled,     // 0 = bypass, 1 = custom DSP active
    
    // Raw inputs from tx_upack
    input wire [15:0] dma_i0,
    input wire [15:0] dma_q0,
    input wire [15:0] dma_i1,
    input wire [15:0] dma_q1,
    input wire dma_valid,   // tx_upack/fifo_rd_valid
    
    // DAC valid input from AD9361
    input wire dac_valid,   // axi_ad9361/dac_enable_i0
    
    // Dynamic DSP strobe and config inputs
    input wire strobe_in,           // tx_strobe_gen_0/enable (pulsed at fs rate)
    input wire [7:0] cic_interp,    // tx_cic_interpolation from GPIO TX bits [11:4]
    
    // Input for processing chain (tx_fir)
    output wire [15:0] proc_i,
    output wire [15:0] proc_q,
    output wire proc_valid,         // to tx_fir/s_axis_data_tvalid
    
    // Read enable to tx_upack
    output wire fifo_rd_en,
    
    // Output from processing chain (tx_cmpy)
    input wire [15:0] modulated_i,
    input wire [15:0] modulated_q,
    
    // Outputs to DAC (axi_ad9361)
    output reg [15:0] dac_i0,
    output reg [15:0] dac_q0,
    output reg [15:0] dac_i1,
    output reg [15:0] dac_q1
);

    // ------------------------------------------------------------------
    // Dynamic Feed Strobe Generator (divides strobe_in by 4 * cic_interp)
    // Total interpolation = 4 (FIR x4) * cic_interp (CIC xN)
    // ------------------------------------------------------------------
    wire [11:0] total_interp = {cic_interp, 2'b00}; // cic_interp * 4
    reg [11:0] strobe_cnt = 12'd0;
    reg feed_strobe = 1'b0;

    always @(posedge clk) begin
        if (!enabled) begin
            strobe_cnt <= 12'd0;
            feed_strobe <= 1'b0;
        end else if (strobe_in) begin
            if (strobe_cnt >= (total_interp - 1'b1)) begin
                strobe_cnt <= 12'd0;
                feed_strobe <= 1'b1;
            end else begin
                strobe_cnt <= strobe_cnt + 12'd1;
                feed_strobe <= 1'b0;
            end
        end else begin
            feed_strobe <= 1'b0;
        end
    end

    // ------------------------------------------------------------------
    // Pacing & Flow Control Multiplexing
    // ------------------------------------------------------------------
    // In bypass mode, fifo_rd_en is driven by dac_valid.
    // In active mode, fifo_rd_en is driven by the dynamic feed_strobe.
    assign fifo_rd_en = enabled ? feed_strobe : dac_valid;

    // FIR input tvalid is driven by tx_upack's fifo_rd_valid when active, 0 in bypass.
    assign proc_valid = enabled ? dma_valid : 1'b0;

    // Data path to processing chain is combinational from selected DMA channel
    assign proc_i = sel ? dma_i1 : dma_i0;
    assign proc_q = sel ? dma_q1 : dma_q0;

    // ------------------------------------------------------------------
    // DAC Output Multiplexing
    // ------------------------------------------------------------------
    always @(posedge clk) begin
        if (!enabled) begin
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
