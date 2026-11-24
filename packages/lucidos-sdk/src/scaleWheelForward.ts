
import { onHostPush, postToHost } from './_bridge';

export const FORWARD_WHEEL_TYPE = 'lucidos:wheel';

export const SCALE_WHEEL_CHANNEL = 'scale-wheel';

export interface ForwardedWheel {
  type: typeof FORWARD_WHEEL_TYPE;
  deltaY: number;
  deltaMode: number;
  metaKey: boolean;
  ctrlKey: boolean;
  modifierKeyHeld: boolean;
}

export function installScaleWheelForwarding(): () => void {
  if (typeof window === 'undefined' || window.parent === window) return () => {};
  let modifierKeyHeld = false;
  let hostArmed = false;
  let attached = false;

  function onWheel(e: WheelEvent) {
    if (!(e.metaKey || e.ctrlKey)) {
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
