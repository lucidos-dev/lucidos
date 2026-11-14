/**
 * Read the pairing QR with this device's own camera.
 *
 * The last of the three ways a code reaches an installed app. The manifest
 * carries it into a fresh install, and the pasteboard carries it into an
 * existing one. Neither reaches an app opened days later, with a QR still on
 * the host's screen. This does.
 *
 * Loaded through `lazyComponent`, so `jsqr` sits in this chunk rather than in
 * the entry bundle. The pairing screen is the first paint of a cold launch, and
 * the one screen an unpaired device can reach. Nothing optional belongs ahead
 * of it.
 *
 * WebKit ships no `BarcodeDetector`, and iOS is the platform this exists for.
 * So one decoder everywhere beats a native path plus a fallback. That would be
 * two behaviours, one of them untested on the device that matters.
 */

import { useEffect, useRef, useState } from 'preact/hooks';
import jsQR from 'jsqr';
import { pairingCodeFromText } from '../../utils/pairingCodeText';

/** How often a frame is decoded. jsQR costs milliseconds per frame on a phone.
 *  A QR held up to a camera is not moving, so eight looks a second finds it at
 *  once and leaves the CPU alone. */
const SCAN_INTERVAL_MS = 125;

/** The longest edge a frame is scaled to before decoding. jsQR walks every
 *  pixel, so a full-resolution rear-camera frame is an order of magnitude of
 *  work for detail a QR does not need. */
const MAX_SCAN_EDGE = 640;

type ScanState = 'opening' | 'scanning' | 'failed';

/** End a camera stream, every track of it.
 *
 *  Exported so the one thing that must never be skipped is testable on its own.
 *  A stream is a list, and stopping the first track leaves the camera live: the
 *  phone keeps its recording indicator lit and the user can see the leak. */
export function stopTracks(stream: MediaStream | null): void {
  stream?.getTracks().forEach((track) => track.stop());
}

export default function PairingScanner({ onCode }: { onCode: (code: string) => void }) {
  const videoRef = useRef<HTMLVideoElement>(null);
  const streamRef = useRef<MediaStream | null>(null);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);
  const timerRef = useRef<number | null>(null);
  // Held in a ref as well as in state: the decode loop is a plain interval, so
  // it cannot read a state closure created before it.
  const doneRef = useRef(false);
  const [state, setState] = useState<ScanState>('opening');

  useEffect(() => {
    /** Every exit runs this. A live camera keeps the phone's recording
     *  indicator lit, so a leaked stream is a bug the user can see. */
    function stop() {
      if (timerRef.current !== null) {
        window.clearInterval(timerRef.current);
        timerRef.current = null;
      }
      stopTracks(streamRef.current);
      streamRef.current = null;
    }

    function tick() {
      if (doneRef.current) return;
      const found = decodeFrame(videoRef.current, canvasRef);
      if (!found) return;
      // Stopped before the callback, not after. The parent unmounts us on a
      // code, and an interval that outlived it would decode into a dead tree.
      doneRef.current = true;
      stop();
      onCode(found);
    }

    navigator.mediaDevices.getUserMedia({ video: { facingMode: 'environment' } }).then(
      (stream) => {
        // Unmounted while the permission prompt was up. The stream still
        // arrives, and nothing else will ever stop it.
        if (doneRef.current) {
          stopTracks(stream);
          return;
        }
        streamRef.current = stream;
        if (videoRef.current) videoRef.current.srcObject = stream;
        setState('scanning');
        timerRef.current = window.setInterval(tick, SCAN_INTERVAL_MS);
      },
      () => {
        if (doneRef.current) return;
        setState('failed');
      },
    );

    return () => {
      doneRef.current = true;
      stop();
    };
  }, []);

  if (state === 'failed') {
    return (
      <p class="pairing-error">
        Lucidos could not open the camera. Allow camera access for this app, or type the code
        instead.
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

/**
 * Decode one frame, or `null` when this one held no pairing code.
 *
 * Split out of the component so the frame arithmetic is readable, and so the
 * downscale cannot quietly be dropped from the hot path.
 */
function decodeFrame(
  video: HTMLVideoElement | null,
  canvasRef: { current: HTMLCanvasElement | null },
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
  // iOS refuses a 2D context once its per-tab canvas budget is spent, the same
  // failure the composer's camera handles. Here the next tick simply retries.
  const ctx = canvas.getContext('2d', { willReadFrequently: true });
  if (!ctx) return null;
  ctx.drawImage(video, 0, 0, w, h);
  const frame = ctx.getImageData(0, 0, w, h);
  // Our QR is dark on white, which is the only orientation a scanner should
  // assume. Trying the inverse doubles the work for a code we render ourselves.
  const result = jsQR(frame.data, frame.width, frame.height, { inversionAttempts: 'dontInvert' });
  return result?.data ? pairingCodeFromText(result.data) : null;
}
