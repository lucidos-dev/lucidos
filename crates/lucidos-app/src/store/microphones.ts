import { signal, type Signal } from '@preact/signals';
import { showToast } from './store';
import { type Loadable, failedIfFresh, setLoadingIfFresh } from './types';
import { microphoneConstraints } from '../voice/microphone';
import { microphoneRefusal } from '../voice/refusals';

export const NO_DEVICE_API = 'This browser will not list audio devices here.';

export const microphones: Signal<Loadable<MediaDeviceInfo[]>> = signal({ status: 'not-loaded' });

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

export function watchMicrophones(): () => void {
  const devices = navigator.mediaDevices;
  if (!devices?.addEventListener) return () => undefined;
  const onChange = (): void => {
    void refreshMicrophones();
  };
  devices.addEventListener('devicechange', onChange);
  return () => devices.removeEventListener('devicechange', onChange);
}

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
