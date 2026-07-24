class TxProcessor extends AudioWorkletProcessor {
  constructor() {
    super();
    this.buffer = new Float32Array(2048);
    this.offset = 0;
  }

  process(inputs, outputs, parameters) {
    const input = inputs[0];
    if (input && input.length > 0) {
      const channelData = input[0];
      this.buffer.set(channelData, this.offset);
      this.offset += channelData.length;
      if (this.offset >= 2048) {
        const sendBuf = this.buffer.slice(0, 2048);
        this.port.postMessage(sendBuf.buffer, [sendBuf.buffer]);
        this.offset = 0;
      }
    }
    return true;
  }
}

registerProcessor('tx-processor', TxProcessor);

