
import { useEffect, useRef, useState } from 'preact/hooks';
import type jsQR from 'jsqr';
import { pairingCodeFromText } from '../../utils/pairingCodeText';
import { lazyModule } from '../../utils/lazyComponent';

const decoder = lazyModule(() => import('jsqr'));

const SCAN_INTERVAL_MS = 125;

const MAX_SCAN_EDGE = 640;

type ScanState = 'opening' | 'scanning' | 'failed';

export function stopTracks(stream: MediaStream | null): void {
  stream?.getTracks().forEach((track) => track.stop());
}

export default function PairingScanner({ onCode }: { onCode: (code: string) => void }) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const timerRef = useRef<number | null>(null);
  const [state, setState] = useState<ScanState>('opening');

  useEffect(() => {
    let decode: typeof jsQR;
    let done = false;
    function stop() {
      if (timerRef.current !== null) {
        window.clearInterval(timerRef.current);
        timerRef.current = null;
      }
      stopTracks(streamRef.current);
      streamRef.current = null;
    }

    function tick() {
      if (done) return;
      const found = decodeFrame(videoRef.current, canvasRef, decode);
      if (!found) return;
      done = true;
      stop();
      onCode(found);
    }

    async function start() {
      try {
        if (!await decoder.preload()) {
          if (!done) setState('failed');
          return;
        }
        if (done) return;
        decode = decoder.get().default;
        const stream = await navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } });
        if (done) {
          stopTracks(stream);
          return;
        }
        streamRef.current = stream;
        if (videoRef.current) videoRef.current.srcObject = stream;
        setState('scanning');
        timerRef.current = window.setInterval(tick, SCAN_INTERVAL_MS);
      } catch {
        stop();
        if (!done) setState('failed');
      }
    }
    void start();

    return () => {
      done = true;
      stop();
    };
  }, []);

  if (state === 'failed') {
    return (
      <p class="pairing-error">
        Lucidos could not start the camera scanner. Check camera access and try again, or type
        the code instead.
      </p>
    );
  }
  return (
    <video
      ref={videoRef}
      class="pairing-scanner-video"
      autoPlay
      playsInline
      muted
      aria-label="Camera viewfinder"
    />
  );
}

function decodeFrame(
  video: HTMLVideoElement | null,
  canvasRef: { current: HTMLCanvasElement | null },
  decode: typeof jsQR,
): string | null {
  const width = video?.videoWidth ?? 0;
  const height = video?.videoHeight ?? 0;
  if (!video || !width || !height) return null;

  const scale = Math.min(1, MAX_SCAN_EDGE / Math.max(width, height));
  const w = Math.round(width * scale);
  const h = Math.round(height * scale);
  const canvas = (canvasRef.current ??= document.createElement('canvas'));
  canvas.width = w;
  canvas.height = h;
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(video, 0, 0, w, h);
  const frame = ctx.getImageData(0, 0, w, h);
  const result = decode(frame.data, frame.width, frame.height, { inversionAttempts: 'dontInvert' });
  return result?.data ? pairingCodeFromText(result.data) : null;
}
