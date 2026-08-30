/**
 * Which microphone a call opens, as pure rules over what the browser reports.
 *
 * `ports.ts` owns the device itself. This owns the two decisions around it:
 * what to constrain `getUserMedia` with, and what the picker draws. Both are
 * rules, so both are tested with no audio hardware anywhere.
 *
 * **Nothing named means the system default.** That is what a call did before
 * this module existed, and it is still what an untouched workspace gets.
 */
import { CHANNELS } from './pcm';
import { refusalName } from './refusals';

/** The stored value meaning "whatever the system calls the default". */
export const SYSTEM_DEFAULT_MICROPHONE = '';

/** What the picker calls that choice. */
export const SYSTEM_DEFAULT_LABEL = 'System default';

/** What the picker calls a chosen microphone that is not connected now. */
export const MISSING_MICROPHONE_LABEL = 'Your choice (not connected)';

/** What it calls that choice when it cannot tell whether it is connected. */
export const UNNAMED_MICROPHONE_LABEL = 'Your choice';

/** Said when the chosen microphone was gone and the call took the default.
 *
 *  It names no device, because there is none to name: the browser refused the
 *  id, and a label for it was never stored. Inventing one would be worse than
 *  saying plainly that the choice did not resolve. */
export const CHOSEN_MICROPHONE_GONE =
  'The microphone you chose is not available, so this call is using the system default.';

/**
 * Chrome's pseudo-devices for "follow the system".
 *
 * Dropped from the picker, because its first row already IS that choice. Two
 * rows meaning one thing is the worse list, and the duplicate reads as a second
 * physical microphone that does not exist.
 */
const PSEUDO_DEVICE_IDS = new Set(['default', 'communications']);

/**
 * The audio constraint one call opens with.
 *
 * No sample rate is asked of the device. The context runs at 24 kHz and
 * resamples the stream itself. Asking hardware for a rate it does not have is
 * an `OverconstrainedError`, and no call at all.
 *
 * The device is pinned with `exact`, never `ideal`. An id that resolves to
 * nothing must REFUSE, so the caller can say the chosen microphone is gone.
 * `ideal` would open a different one and tell nobody.
 */
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

/**
 * Did this refusal mean the NAMED device is gone, rather than no audio at all?
 *
 * Two names, because browsers disagree on which one an unresolvable id raises.
 * Asked only when a device WAS named, so a machine with no microphone still
 * reports the truth: the retry underneath refuses too, and that second refusal
 * is the one the reader gets.
 *
 * The name is read off the object by {@link refusalName}, never behind an
 * `instanceof DOMException`. `OverconstrainedError` has its own interface, so
 * that gate would answer `''` for the one refusal this exists to catch.
 */
export function namedDeviceIsGone(err: unknown): boolean {
  const name = refusalName(err);
  return name === 'OverconstrainedError' || name === 'NotFoundError';
}

/**
 * Open the chosen microphone, or the system default when it is gone.
 *
 * The `ask` effect is injected, so this rule is driven by a stub in a test and
 * by `getUserMedia` in the browser. What it decides is which of two refusals
 * the reader ends up hearing about.
 *
 * A fallback is REPORTED, never swallowed: it comes back as the note, and the
 * runner says it out loud. Recording from an unasked-for microphone in silence
 * is the failure this whole picker exists to end.
 */
export async function openWithFallback(
  deviceId: string | null,
  ask: (id: string | null) => Promise<MediaStream>,
): Promise<{ stream: MediaStream; note: string | null }> {
  try {
    return { stream: await ask(deviceId), note: null };
  } catch (err) {
    // A denied permission, a busy device or no audio at all is the real
    // answer. Asking again without the id would only collect it twice.
    if (!deviceId || !namedDeviceIsGone(err)) throw err;
  }
  // Whatever this throws is the honest refusal. A machine with no microphone at
  // all reaches here, and blaming the choice would send the reader to a picker
  // that cannot help them.
  return { stream: await ask(null), note: CHOSEN_MICROPHONE_GONE };
}

/** One row of the microphone picker. */
export interface MicrophoneChoice {
  value: string;
  label: string;
}

/**
 * Did this browser name a single microphone?
 *
 * Until the page has been granted the microphone once, `enumerateDevices`
 * reports every input with an EMPTY label. Where the permission is blocked
 * outright it reports no input at all. Both leave us unable to name anything,
 * so the picker offers the grant rather than numbering rows it cannot name.
 */
export function anyMicrophoneNamed(devices: readonly MediaDeviceInfo[]): boolean {
  return devices.some(
    (d) =>
      d.kind === 'audioinput' && !PSEUDO_DEVICE_IDS.has(d.deviceId) && d.label.trim() !== '',
  );
}

/**
 * The picker's rows: the system default, then every microphone the browser
 * will name.
 *
 * A device the browser will not name is dropped rather than numbered. When
 * {@link anyMicrophoneNamed} is false none of them survived, and the picker
 * offers the grant instead.
 *
 * A `stored` id no row carries is ALWAYS appended, so the reader can see and
 * change their choice whatever the browser is willing to say. It is the only
 * row that can be checked. Drop it and the picker claims nothing is chosen,
 * while the next call opens on that very device.
 *
 * What varies is what it is CALLED. "Not connected" is a claim, and only a
 * list that named something can support it. Read an empty list, or one whose
 * labels are all blank, and the device may be plugged in and about to answer.
 */
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
