// burst_gate.v
// Gates a continuous IQ write stream into fixed-length bursts for the waterfall
// DMA path. On a rising edge of `trigger` it passes exactly BURST_LEN samples
// downstream (marking the first with a sync), then stops until the next trigger.
// When `enabled` = 0 it is transparent (pass-through of data, write-enable and sync).
// Used to capture a bounded spectrum snapshot without free-running the DMA.

`timescale 1ns / 1ps

module burst_gate #(
    parameter integer DATA_WIDTH = 64, // 64-bit for PlutoSDR wideband path (2 channels of I/Q)
    // Must match `WATERFALL_DMA_SIZE` (16384) in the Rust backend (src/main.rs)
    parameter integer BURST_LEN = 16384
)(
    input wire clk,
    (* X_INTERFACE_IGNORE = "TRUE" *)
    input wire rst_n,
    
    // Control inputs
    (* X_INTERFACE_IGNORE = "TRUE" *)
    input wire trigger,    // Rising edge triggers a new burst
    (* X_INTERFACE_IGNORE = "TRUE" *)
    input wire enabled,    // 1 = Gated burst mode enabled, 0 = Gated burst mode disabled (pass-through)
    
    // Data input
    input wire [DATA_WIDTH-1:0] s_fifo_wr_data,
    input wire s_fifo_wr_en,
    input wire s_fifo_wr_sync,
    
    // Data output
    output wire [DATA_WIDTH-1:0] m_fifo_wr_data,
    output wire m_fifo_wr_en,
    output wire m_fifo_wr_sync
);

    // Just wide enough to count 0..BURST_LEN-1
    localparam integer CNT_W = $clog2(BURST_LEN);

    reg active;
    reg [CNT_W-1:0] count;
    reg prev_trigger;

    // Detect rising edge on trigger input
    always @(posedge clk) begin
        if (!rst_n) begin
            prev_trigger <= 1'b0;
        end else begin
            prev_trigger <= trigger;
        end
    end
    
    wire trigger_pulse = trigger && !prev_trigger;

    // Burst Control State Machine
    always @(posedge clk) begin
        if (!rst_n) begin
            active <= 1'b0;
            count  <= {CNT_W{1'b0}};
        end else if (!enabled) begin
            active <= 1'b0;
            count  <= {CNT_W{1'b0}};
        end else begin
            if (!active) begin
                if (trigger_pulse) begin
                    active <= 1'b1;
                    count  <= {CNT_W{1'b0}};
                end
            end else begin
                if (s_fifo_wr_en) begin
                    if (count >= (BURST_LEN - 1)) begin
                        // Let this last sample pass through (active is still 1 this cycle,
                        // so m_fifo_wr_en fires), then deactivate on the next cycle.
                        active <= 1'b0;
                        count  <= {CNT_W{1'b0}};
                    end else begin
                        count <= count + 1'b1;
                    end
                end
            end
        end
    end

    // Master FIFO routing
    assign m_fifo_wr_data = s_fifo_wr_data;
    
    // Only pass writes downstream when active or when gated burst is disabled (pass-through)
    assign m_fifo_wr_en   = !enabled ? s_fifo_wr_en : (active && s_fifo_wr_en);
    
    // Generate sync on the first sample of the burst to keep the DMA aligned
    assign m_fifo_wr_sync = !enabled ? s_fifo_wr_sync : (active && (count == 0) && s_fifo_wr_en);

endmodule
