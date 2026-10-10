/**
 * The microphones this browser will admit to.
 *
 * Hardware, not server state, so it is read from `enumerateDevices` rather than
 * from a frame. It is refreshed when the picker is about to open, and again
 * whenever the browser reports the set changed.
 *
 * The rules over the answer live in `voice/microphone.ts`, so this module is
 * only the read and the signal holding it.
 */
import { signal, type Signal } from '@preact/signals';
import { showToast } from './store';
import { type Loadable, failedIfFresh, setLoadingIfFresh } from './types';
import { microphoneConstraints } from '../voice/microphone';
import { microphoneRefusal } from '../voice/refusals';

/** Said when the browser cannot enumerate at all, which is an insecure origin. */
export const NO_DEVICE_API = 'This browser will not list audio devices here.';

/** Every audio device the browser reports, inputs and outputs alike.
 *
 *  Unfiltered on purpose: `microphoneChoices` owns which rows a picker draws,
 *  and `anyMicrophoneNamed` has to weigh the inputs itself. */
export const microphones: Signal<Loadable<MediaDeviceInfo[]>> = signal({ status: 'not-loaded' });

/**
 * Re-read the device list.
 *
 * A refresh keeps the list it already had while it runs, so reopening the
 * picker does not blank the rows the reader is looking at.
 */
export async function refreshMicrophones(): Promise<void> {
  if (!navigator.mediaDevices?.enumerateDevices) {
    microphones.value = failedIfFresh(microphones.peek(), new Error(NO_DEVICE_API));
    return;
  }
  setLoadingIfFresh(microphones);
  try {
    microphones.value = { status: 'loaded', data: await navigator.mediaDevices.enumerateDevices() };
  } catch (err) {
    microphones.value = failedIfFresh(microphones.peek(), err);
  }
}

/**
 * Watch for a microphone being plugged in or unplugged.
 *
 * Returns the way to stop watching. The picker subscribes while it is open and
 * drops it on close, so a menu nobody opened costs no listener.
 */
export function watchMicrophones(): () => void {
  const devices = navigator.mediaDevices;
  if (!devices?.addEventListener) return () => undefined;
  const onChange = (): void => {
    void refreshMicrophones();
  };
  devices.addEventListener('devicechange', onChange);
  return () => devices.removeEventListener('devicechange', onChange);
}

/**
 * Ask for the microphone once, purely so the browser will name the devices.
 *
 * Until a page has been granted the microphone, `enumerateDevices` reports
 * blank labels. So a reader who has never placed a call sees a list it cannot
 * describe. This opens the default input and closes it again immediately: the
 * grant is the whole point, and the stream is not.
 */
export async function nameMicrophones(): Promise<void> {
  try {
    const stream = await navigator.mediaDevices.getUserMedia({
      audio: microphoneConstraints(null),
    });
    stream.getTracks().forEach((track) => track.stop());
  } catch (err) {
    showToast(microphoneRefusal(err), 'error');
    return;
  }
  await refreshMicrophones();
}
