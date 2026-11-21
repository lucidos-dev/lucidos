// Forward Cmd/Ctrl + wheel out of app iframes, so it scales the host UI.
//
// A wheel over an app frame lands in the frame's own document, where the
// host's scale listener (`scaleWheelListener.ts`) cannot see it. With focus in
// the frame, the host also never sees the modifier's keydown. So the gesture
// that scales the UI everywhere else silently died over an app.
//
// The listener cancels the browser's own zoom, so it must be non-passive. That
// makes every scroll in the app wait on its main thread. So it is attached only
// while a wheel can be a zoom: this frame saw the modifier go down, or the host
// says it is listening (`armed` on the `scale-wheel` channel). The second case
// covers a modifier held with focus in the shell, and a pinch while the scale
// panel is open.
//
// An app that claims the gesture for itself (`preventDefault`, say to zoom a
// map) keeps it. The listener sits on the window in the bubble phase, so the
// app's own element handlers have run by then.

import { onHostPush, postToHost } from './_bridge';

/** Wire type for a forwarded wheel. The host imports this constant. */
export const FORWARD_WHEEL_TYPE = 'lucidos:wheel';

/** Push channel saying whether the host's scale listener is attached. The host
 *  imports this constant, so both ends speak one name. */
export const SCALE_WHEEL_CHANNEL = 'scale-wheel';

export interface ForwardedWheel {
  type: typeof FORWARD_WHEEL_TYPE;
  deltaY: number;
  deltaMode: number;
  metaKey: boolean;
  ctrlKey: boolean;
  /** This frame saw the modifier's keydown. A pinch arrives flagged ctrl with
   *  no key down, and the host steps it differently. */
  modifierKeyHeld: boolean;
}

/** Install the forwarder for the current iframe. Call once on SDK load.
 *  Returns the teardown. No-op at the top level, with no host to tell. */
export function installScaleWheelForwarding(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  let modifierKeyHeld = false;
  let hostArmed = false;
  let attached = false;

  function onWheel(e: WheelEvent) {
    if (!(e.metaKey || e.ctrlKey)) {
      // The modifier's keyup went elsewhere, such as to the host.
      modifierKeyHeld = false;
      sync();
      return;
    }
    if (e.defaultPrevented) return;
    e.preventDefault();
    const message: ForwardedWheel = {
      type: FORWARD_WHEEL_TYPE,
      deltaY: e.deltaY,
      deltaMode: e.deltaMode,
      metaKey: e.metaKey,
      ctrlKey: e.ctrlKey,
      modifierKeyHeld,
    };
    postToHost(message);
  }

  function sync() {
    const wanted = modifierKeyHeld || hostArmed;
    if (wanted === attached) return;
    attached = wanted;
    if (wanted) window.addEventListener('wheel', onWheel, { passive: false });
    else window.removeEventListener('wheel', onWheel);
  }

  function trackModifier(e: KeyboardEvent) {
    modifierKeyHeld = e.metaKey || e.ctrlKey;
    sync();
  }

  function releaseModifier() {
    modifierKeyHeld = false;
    sync();
  }

  const offArmed = onHostPush(SCALE_WHEEL_CHANNEL, (data) => {
    hostArmed = (data as { armed?: unknown } | null)?.armed === true;
    sync();
  });
  window.addEventListener('keydown', trackModifier, true);
  window.addEventListener('keyup', trackModifier, true);
  window.addEventListener('blur', releaseModifier);
  return () => {
    window.removeEventListener('keydown', trackModifier, true);
    window.removeEventListener('keyup', trackModifier, true);
    window.removeEventListener('blur', releaseModifier);
    window.removeEventListener('wheel', onWheel);
    offArmed();
  };
}
