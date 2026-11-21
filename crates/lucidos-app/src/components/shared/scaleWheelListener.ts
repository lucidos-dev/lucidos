import { computed, effect, signal } from '@preact/signals';
import { UI_SCALE_STEP } from '../../store/actions/preferences';
import { postToAppFrame } from '../../store/actions/app-frame-channel';
import { appFrameFor, mountedAppFrames } from '../../utils/appFrame';
import { nowMs } from '../../utils/scrollActivity';
// The SDK module that forwards the wheel. Relative, because the barrel does not
// export it; it has no import-time side effects.
import {
  FORWARD_WHEEL_TYPE, SCALE_WHEEL_CHANNEL, type ForwardedWheel,
} from '../../../../../packages/lucidos-sdk/src/scaleWheelForward';
import { createWheelZoom } from './scaleWheel';
import { adjustUiScale, renewScaleModalLinger, scaleModalOpen } from './scaleModalState';

/** Whether app frames should arm their wheel forwarder. Written by
 *  `installScaleWheel`, read by a frame as it loads. */
const framesArmed = signal(false);

/** Hand one app frame the armed state. Called as the frame loads, since a
 *  push to a document that has not run the SDK yet reaches nobody. */
export function pushScaleWheelArmedToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, SCALE_WHEEL_CHANNEL, { armed: framesArmed.peek() });
}

/**
 * Cmd/Ctrl + wheel steps the UI scale instead of zooming the page, with the
 * scale panel open or closed, the same as Cmd +/-. The first step opens the
 * panel, and releasing the modifier dismisses it.
 *
 * The listener has to be non-passive to cancel the browser's own zoom. A
 * non-passive `document` wheel listener makes every scroll wait on the main
 * thread, which janks a long transcript. So it is attached only while a wheel
 * can be a zoom: a modifier is held, or the panel is open. The second case
 * covers a trackpad pinch, which Chrome sends as ctrl-wheel with no keydown.
 * Over an app frame, the SDK forwards the wheel here by the same rule.
 *
 * Returns the teardown.
 */
export function installScaleWheel(): () => void {
  const zoom = createWheelZoom(dir => adjustUiScale(dir * UI_SCALE_STEP));
  const modifierHeld = signal(false);

  function trackModifier(e: KeyboardEvent) {
    modifierHeld.value = e.metaKey || e.ctrlKey;
  }

  function releaseModifier() {
    modifierHeld.value = false;
  }

  function handleWheel(e: WheelEvent) {
    // A surface that zooms its own content (the image popup) claimed it first.
    if (e.defaultPrevented) return;
    if (!(e.metaKey || e.ctrlKey)) {
      // The modifier's keyup went elsewhere, such as into an iframe.
      releaseModifier();
      return;
    }
    e.preventDefault();
    // A pinch arrives flagged ctrl, never meta, and fires no keydown. So meta,
    // or a ctrl whose keydown the window saw, is a real key.
    zoomBy(e.deltaY, e.deltaMode, e.metaKey || modifierHeld.value);
  }

  function zoomBy(deltaY: number, deltaMode: number, modifierKeyHeld: boolean) {
    // The gesture is the user working the control, whether or not it has
    // travelled a whole notch yet. A slow pinch may not step for several frames.
    renewScaleModalLinger();
    zoom.push(deltaY, deltaMode, nowMs(), modifierKeyHeld);
  }

  // A wheel over an app frame lands in the frame's document. The SDK forwards
  // the zoom-shaped ones (scaleWheelForward.ts), already cancelled there.
  function handleForwardedWheel(e: MessageEvent) {
    const data = e.data as Partial<Record<keyof ForwardedWheel, unknown>> | null;
    if (!data || typeof data !== 'object' || data.type !== FORWARD_WHEEL_TYPE) return;
    if (!appFrameFor(e.source)) return;
    if (typeof data.deltaY !== 'number' || typeof data.deltaMode !== 'number') return;
    const metaKey = data.metaKey === true;
    if (!(metaKey || data.ctrlKey === true)) return;
    // The keydown may have landed in the shell, not the frame.
    zoomBy(data.deltaY, data.deltaMode, metaKey || data.modifierKeyHeld === true || modifierHeld.value);
  }

  // Re-runs only when the answer flips, so a modifier pressed mid-pinch cannot
  // stop the gesture it joins.
  const listening = computed(() => modifierHeld.value || scaleModalOpen.value);

  // Synchronous on the signal write, so the listener exists before the panel's
  // first paint. A pinch in that gap would fall through to the browser's zoom.
  const stopListening = effect(() => {
    if (!listening.value) return;
    document.addEventListener('wheel', handleWheel, { passive: false });
    return () => {
      document.removeEventListener('wheel', handleWheel);
      // A gesture nothing listens to any more is over. A frame still queued,
      // say in a tab sent to the background, would open the panel later.
      zoom.stop();
    };
  });

  // The cleanup runs when the panel closes. A step drained after that, from
  // Escape with the modifier still held, would reopen it.
  const stopOnClose = effect(() => {
    if (!scaleModalOpen.value) return;
    return () => zoom.stop();
  });

  // Tell every app frame when a wheel can be a zoom, so the frame arms its own
  // listener. A modifier held in the shell, or a pinch with the panel open, can
  // land over an app.
  const stopArmingFrames = effect(() => {
    const armed = listening.value;
    framesArmed.value = armed;
    for (const frame of mountedAppFrames()) postToAppFrame(frame, SCALE_WHEEL_CHANNEL, { armed });
  });

  window.addEventListener('keydown', trackModifier, true);
  window.addEventListener('keyup', trackModifier, true);
  window.addEventListener('blur', releaseModifier);
  window.addEventListener('message', handleForwardedWheel);
  return () => {
    window.removeEventListener('keydown', trackModifier, true);
    window.removeEventListener('keyup', trackModifier, true);
    window.removeEventListener('blur', releaseModifier);
    window.removeEventListener('message', handleForwardedWheel);
    stopListening();
    stopOnClose();
    stopArmingFrames();
    framesArmed.value = false;
  };
}
