// Binary WebSocket frame layout, mirrored from `msg_header` in src/threads/network.rs: keep the
// two in sync when adding a frame type. Byte 0 is the frame type, bytes 1-3 are reserved (zero),
// payload starts at byte HEADER_BYTES.

export const FRAME_TYPE = {
  WATERFALL: 0,  // server -> client: one u8-per-bin waterfall row
  AUDIO: 1,      // server -> client: demodulated RX audio, f32 LE PCM
  TX_AUDIO: 2,   // client -> server: TX audio, f32 LE PCM
  IQ: 3,         // server -> client: raw interleaved i16 LE I/Q (opt-in)
};

export const HEADER_BYTES = 4;

// Wraps a TypedArray payload in a headered frame ready for WebSocket.send().
export function encodeFrame(type, payload) {
  const buffer = new ArrayBuffer(HEADER_BYTES + payload.byteLength);
  new DataView(buffer).setUint8(0, type);
  new Uint8Array(buffer, HEADER_BYTES)
    .set(new Uint8Array(payload.buffer, payload.byteOffset, payload.byteLength));
  return buffer;
}
