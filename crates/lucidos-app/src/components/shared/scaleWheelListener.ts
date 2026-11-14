import { computed, effect, signal } from '@preact/signals';
import { UI_SCALE_STEP } from '../../store/actions/preferences';
import { nowMs } from '../../utils/scrollActivity';
import { createWheelZoom } from './scaleWheel';
import { adjustUiScale, renewScaleModalLinger, scaleModalOpen } from './scaleModalState';

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
    // The gesture is the user working the control, whether or not it has
    // travelled a whole notch yet. A slow pinch may not step for several frames.
    renewScaleModalLinger();
    // A pinch arrives flagged ctrl, never meta, and fires no keydown. So meta,
    // or a ctrl whose keydown the window saw, is a real key.
    zoom.push(e.deltaY, e.deltaMode, nowMs(), e.metaKey || modifierHeld.value);
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

  window.addEventListener('keydown', trackModifier, true);
  window.addEventListener('keyup', trackModifier, true);
  window.addEventListener('blur', releaseModifier);
  return () => {
    window.removeEventListener('keydown', trackModifier, true);
    window.removeEventListener('keyup', trackModifier, true);
    window.removeEventListener('blur', releaseModifier);
    stopListening();
    stopOnClose();
  };
}
