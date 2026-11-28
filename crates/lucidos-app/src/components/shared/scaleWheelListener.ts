import { computed, effect, signal } from '@preact/signals';
import { UI_SCALE_STEP } from '../../store/actions/preferences';
import { postToAppFrame } from '../../store/actions/app-frame-channel';
import { appFrameFor, mountedAppFrames } from '../../utils/appFrame';
import { nowMs } from '../../utils/scrollActivity';
import {
  FORWARD_WHEEL_TYPE, SCALE_WHEEL_CHANNEL, type ForwardedWheel,
} from '../../../../../packages/lucidos-sdk/src/scaleWheelForward';
import { createWheelZoom } from './scaleWheel';
import { adjustUiScale, renewScaleModalLinger, scaleModalOpen } from './scaleModalState';

const framesArmed = signal(false);

export function pushScaleWheelArmedToFrame(frame: HTMLIFrameElement): void {
  postToAppFrame(frame, SCALE_WHEEL_CHANNEL, { armed: framesArmed.peek() });
}

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
    if (e.defaultPrevented) return;
    if (!(e.metaKey || e.ctrlKey)) {
      releaseModifier();
      return;
    }
    e.preventDefault();
    zoomBy(e.deltaY, e.deltaMode, e.metaKey || modifierHeld.value);
  }

  function zoomBy(deltaY: number, deltaMode: number, modifierKeyHeld: boolean) {
    renewScaleModalLinger();
    zoom.push(deltaY, deltaMode, nowMs(), modifierKeyHeld);
  }

  function handleForwardedWheel(e: MessageEvent) {
    const data = e.data as Partial<Record<keyof ForwardedWheel, unknown>> | null;
    if (!data || typeof data !== 'object' || data.type !== FORWARD_WHEEL_TYPE) return;
    if (!appFrameFor(e.source)) return;
    if (typeof data.deltaY !== 'number' || typeof data.deltaMode !== 'number') return;
    const metaKey = data.metaKey === true;
    if (!(metaKey || data.ctrlKey === true)) return;
    zoomBy(data.deltaY, data.deltaMode, metaKey || data.modifierKeyHeld === true || modifierHeld.value);
  }

  const listening = computed(() => modifierHeld.value || scaleModalOpen.value);

  const stopListening = effect(() => {
    if (!listening.value) return;
    document.addEventListener('wheel', handleWheel, { passive: false });
    return () => {
      document.removeEventListener('wheel', handleWheel);
      zoom.stop();
    };
  });

  const stopOnClose = effect(() => {
    if (!scaleModalOpen.value) return;
    return () => zoom.stop();
  });

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
