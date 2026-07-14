// iq_packer.v
// Combines two 56-bit IQ data streams into one 32-bit output, shifting depending on a previous decimation rate
// Used for combining the outputs of the two CIC-Compilers for further processing

module iq_packer (
    input wire clk,
    input wire resetn,

    input wire [7:0] dec_rate,    // Decimation rate of previous audio path

    // Data input
    input wire signed [55:0] s_i_tdata,
    input wire s_i_tvalid,
    output wire s_i_tready,
    input wire signed [55:0] s_q_tdata,
    input wire s_q_tvalid,
    output wire s_q_tready,

    // Data output
    output wire [31:0] m_tdata,
    output wire m_tvalid,
    input wire m_tready
);
    // --- Registered synchroniser ---
    // Accept a new IQ pair only when BOTH channels are simultaneously valid AND the output stage is empty. 
    reg signed [55:0] i_reg;
    reg signed [55:0] q_reg;
    reg        pending;   // 1 = i_reg/q_reg hold a valid pair awaiting downstream

    wire both_valid   = s_i_tvalid & s_q_tvalid;
    wire can_accept   = !pending | m_tready;   // stage empty, or downstream takes this cycle

    assign s_i_tready = can_accept;
    assign s_q_tready = can_accept;

    // Dynamically calculate the bit shift required based on the CIC decimation rate
    // The CIC Gain is R^4, which is equivalent to a bit shift of 4 * log2(R).
    wire [4:0] dynamic_shift;
    assign dynamic_shift = (dec_rate >= 64) ? 5'd24 :
                           (dec_rate >= 32) ? 5'd20 :
                           (dec_rate >= 16) ? 5'd16 :
                           (dec_rate >= 8)  ? 5'd12 : 5'd8;

    always @(posedge clk) begin
        if (!resetn) begin
            pending <= 1'b0;
        end else begin
            if (pending && m_tready) begin
                // Downstream consumed the staged pair
                pending <= 1'b0;
            end
            if (both_valid && can_accept) begin
                // Latch the new pair
                i_reg   <= s_i_tdata;
                q_reg   <= s_q_tdata;
                pending <= 1'b1;
            end
        end
    end

    wire [55:0] shifted_i = i_reg >>> (12 + dynamic_shift);
    wire [55:0] shifted_q = q_reg >>> (12 + dynamic_shift);

    assign m_tvalid = pending;
    assign m_tdata  = {shifted_q[15:0], shifted_i[15:0]};

endmodule
