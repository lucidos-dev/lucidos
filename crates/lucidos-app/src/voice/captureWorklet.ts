import { CAPTURE_FRAME_SAMPLES } from './pcm';

export const CAPTURE_WORKLET_NAME = 'lucidos-capture';

export const CAPTURE_WORKLET_SOURCE = `
class LucidosCapture extends AudioWorkletProcessor {
  constructor() {
    super();
    this.frame = new Float32Array(${CAPTURE_FRAME_SAMPLES});
    this.filled = 0;
  }

  process(inputs) {
    const channel = inputs[0] && inputs[0][0];
    if (!channel) return true;
    let read = 0;
    while (read < channel.length) {
      const take = Math.min(channel.length - read, this.frame.length - this.filled);
      this.frame.set(channel.subarray(read, read + take), this.filled);
      this.filled += take;
      read += take;
      if (this.filled === this.frame.length) {
        this.port.postMessage(this.frame.slice());
        this.filled = 0;
      }
    }
    return true;
  }
}

registerProcessor(${JSON.stringify(CAPTURE_WORKLET_NAME)}, LucidosCapture);
`;

let cached: string | null = null;

export function captureWorkletUrl(): string {
  if (cached === null) {
    cached = URL.createObjectURL(
      new Blob([CAPTURE_WORKLET_SOURCE], { type: 'text/javascript' }),
    );
  }
  return cached;
}
