/**
 * The microphone worklet, as source text.
 *
 * It runs on the audio thread, where it does one job: gather the 128-sample
 * quanta Web Audio hands it into frames of {@link CAPTURE_FRAME_SAMPLES}, and
 * post each finished frame to the main thread. Conversion, energy and the
 * barge-in gate all live in tested modules on the other side of that message.
 *
 * A string rather than a file, so it needs no Vite asset entry and no
 * base-path handling. `addModule` takes a URL, and a blob URL of this text is
 * one. The frame size is interpolated, so `pcm.ts` stays its one definition.
 */
import { CAPTURE_FRAME_SAMPLES } from './pcm';

/** The name the node is constructed with. */
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

/**
 * A blob URL for the worklet, made once per page.
 *
 * Never revoked. One small blob outlives the page at no cost, and revoking it
 * would break the next call's `addModule` on a fresh `AudioContext`.
 */
export function captureWorkletUrl(): string {
  if (cached === null) {
    cached = URL.createObjectURL(
      new Blob([CAPTURE_WORKLET_SOURCE], { type: 'text/javascript' }),
    );
  }
  return cached;
}
