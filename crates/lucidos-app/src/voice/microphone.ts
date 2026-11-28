import { CHANNELS } from './pcm';
import { refusalName } from './refusals';

export const SYSTEM_DEFAULT_MICROPHONE = '';

export const SYSTEM_DEFAULT_LABEL = 'System default';

export const MISSING_MICROPHONE_LABEL = 'Your choice (not connected)';

export const UNNAMED_MICROPHONE_LABEL = 'Your choice';

export const CHOSEN_MICROPHONE_GONE =
  'The microphone you chose is not available, so this call is using the system default.';

const PSEUDO_DEVICE_IDS = new Set(['default', 'communications']);

export function microphoneConstraints(deviceId: string | null): MediaTrackConstraints {
  const audio: MediaTrackConstraints = {
    echoCancellation: true,
    noiseSuppression: true,
    autoGainControl: true,
    channelCount: CHANNELS,
  };
  if (deviceId) audio.deviceId = { exact: deviceId };
  return audio;
}

export function namedDeviceIsGone(err: unknown): boolean {
  const name = refusalName(err);
  return name === 'OverconstrainedError' || name === 'NotFoundError';
}

export async function openWithFallback(
  deviceId: string | null,
  ask: (id: string | null) => Promise<MediaStream>,
): Promise<{ stream: MediaStream; note: string | null }> {
  try {
    return { stream: await ask(deviceId), note: null };
  } catch (err) {
    if (!deviceId || !namedDeviceIsGone(err)) throw err;
  }
  return { stream: await ask(null), note: CHOSEN_MICROPHONE_GONE };
}

export interface MicrophoneChoice {
  value: string;
  label: string;
}

export function anyMicrophoneNamed(devices: readonly MediaDeviceInfo[]): boolean {
  return devices.some(
    (d) =>
      d.kind === 'audioinput' && !PSEUDO_DEVICE_IDS.has(d.deviceId) && d.label.trim() !== '',
  );
}

export function microphoneChoices(
  devices: readonly MediaDeviceInfo[],
  stored: string,
): MicrophoneChoice[] {
  const rows: MicrophoneChoice[] = [
    { value: SYSTEM_DEFAULT_MICROPHONE, label: SYSTEM_DEFAULT_LABEL },
  ];
  for (const device of devices) {
    if (device.kind !== 'audioinput') continue;
    if (PSEUDO_DEVICE_IDS.has(device.deviceId)) continue;
    const label = device.label.trim();
    if (label === '') continue;
    rows.push({ value: device.deviceId, label });
  }
  const namedSomething = anyMicrophoneNamed(devices);
  const listed = rows.some((row) => row.value === stored);
  if (stored !== SYSTEM_DEFAULT_MICROPHONE && !listed) {
    rows.push({
      value: stored,
      label: namedSomething ? MISSING_MICROPHONE_LABEL : UNNAMED_MICROPHONE_LABEL,
    });
  }
  return rows;
}
