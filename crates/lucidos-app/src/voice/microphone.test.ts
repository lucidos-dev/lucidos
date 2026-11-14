import { describe, it, expect } from 'vitest';
import { CHANNELS } from './pcm';
import {
  CHOSEN_MICROPHONE_GONE,
  MISSING_MICROPHONE_LABEL,
  UNNAMED_MICROPHONE_LABEL,
  SYSTEM_DEFAULT_LABEL,
  SYSTEM_DEFAULT_MICROPHONE,
  microphoneChoices,
  microphoneConstraints,
  anyMicrophoneNamed,
  namedDeviceIsGone,
  openWithFallback,
} from './microphone';

function input(deviceId: string, label: string): MediaDeviceInfo {
  return { deviceId, label, kind: 'audioinput', groupId: 'g' } as MediaDeviceInfo;
}

function output(deviceId: string, label: string): MediaDeviceInfo {
  return { deviceId, label, kind: 'audiooutput', groupId: 'g' } as MediaDeviceInfo;
}

function refusal(name: string): DOMException {
  return new DOMException('no', name);
}

describe('microphoneConstraints', () => {
  /** The whole promise of the default case: a workspace that never opens the
   *  picker asks for exactly what it asked for before the picker existed. */
  it('names no device when nothing is chosen', () => {
    expect(microphoneConstraints(null)).toEqual({
      echoCancellation: true,
      noiseSuppression: true,
      autoGainControl: true,
      channelCount: CHANNELS,
    });
  });

  /** `exact`, so an id that resolves to nothing REFUSES. `ideal` would open a
   *  different microphone and tell nobody, which is the bug being fixed. */
  it('pins a chosen device exactly, leaving the rest alone', () => {
    const audio = microphoneConstraints('mic-1');
    expect(audio.deviceId).toEqual({ exact: 'mic-1' });
    expect(audio.channelCount).toBe(CHANNELS);
    expect(audio.echoCancellation).toBe(true);
  });
});

describe('namedDeviceIsGone', () => {
  it('recognises both spellings browsers use for an id that resolves to nothing', () => {
    expect(namedDeviceIsGone(refusal('OverconstrainedError'))).toBe(true);
    expect(namedDeviceIsGone(refusal('NotFoundError'))).toBe(true);
  });

  /** A denied permission or a busy device is not a missing one. Retrying
   *  without the id would ask the same refusal a second time. */
  it('is not fooled by a refusal that has nothing to do with the id', () => {
    expect(namedDeviceIsGone(refusal('NotAllowedError'))).toBe(false);
    expect(namedDeviceIsGone(refusal('NotReadableError'))).toBe(false);
    expect(namedDeviceIsGone(new Error('boom'))).toBe(false);
  });
});

describe('anyMicrophoneNamed', () => {
  it('is true as soon as one input has a name', () => {
    expect(anyMicrophoneNamed([input('a', ''), input('b', 'Headset')])).toBe(true);
  });

  /** Blank labels before a grant, and no inputs at all where the permission is
   *  blocked. Both leave the picker unable to describe anything, so both have
   *  to reach the same answer. */
  it('is false for blank labels, for no inputs, and for outputs alone', () => {
    expect(anyMicrophoneNamed([input('a', ''), input('b', '')])).toBe(false);
    expect(anyMicrophoneNamed([])).toBe(false);
    expect(anyMicrophoneNamed([output('o', 'Speakers')])).toBe(false);
  });

  /** A pseudo-device is the system default under another name, and the picker
   *  draws that row itself. Counting it would call the list named. */
  it('is false when only a pseudo-device carries a name', () => {
    expect(anyMicrophoneNamed([input('default', 'Default - Built-in')])).toBe(false);
  });
});

describe('openWithFallback', () => {
  const stream = {} as MediaStream;

  /** An asked-for device that opens is the whole of it: no retry, no note. */
  it('opens the chosen device and says nothing', async () => {
    const asked: (string | null)[] = [];
    const opened = await openWithFallback('mic-1', (id) => {
      asked.push(id);
      return Promise.resolve(stream);
    });
    expect(asked).toEqual(['mic-1']);
    expect(opened.note).toBeNull();
  });

  /** The reported bug's cure. The call goes up rather than dying, and the
   *  reader is told which microphone it went up on. */
  it('falls back to the default and says so when the choice is gone', async () => {
    const asked: (string | null)[] = [];
    const opened = await openWithFallback('headset-gone', (id) => {
      asked.push(id);
      if (id !== null) return Promise.reject(refusal('OverconstrainedError'));
      return Promise.resolve(stream);
    });
    expect(asked).toEqual(['headset-gone', null]);
    expect(opened.note).toBe(CHOSEN_MICROPHONE_GONE);
  });

  /** A machine with no microphone must hear about the microphone, not about a
   *  choice it cannot fix by choosing again. */
  it('reports the second refusal when the default is gone too', async () => {
    const denied = refusal('NotFoundError');
    await expect(
      openWithFallback('headset-gone', () => Promise.reject(denied)),
    ).rejects.toBe(denied);
  });

  /** A denied permission is not a missing device, so nothing is retried. */
  it('never retries a refusal the id did not cause', async () => {
    const denied = refusal('NotAllowedError');
    let calls = 0;
    await expect(
      openWithFallback('mic-1', () => {
        calls++;
        return Promise.reject(denied);
      }),
    ).rejects.toBe(denied);
    expect(calls).toBe(1);
  });

  /** With nothing chosen there is nothing to fall back FROM, so the refusal
   *  stands as it always did. */
  it('does not retry when no device was named', async () => {
    const gone = refusal('NotFoundError');
    let calls = 0;
    await expect(
      openWithFallback(null, () => {
        calls++;
        return Promise.reject(gone);
      }),
    ).rejects.toBe(gone);
    expect(calls).toBe(1);
  });
});

describe('microphoneChoices', () => {
  it('leads with the system default and lists every named input', () => {
    const rows = microphoneChoices(
      [input('a', 'MacBook Pro Microphone'), output('o', 'Speakers'), input('b', 'Headset')],
      SYSTEM_DEFAULT_MICROPHONE,
    );
    expect(rows).toEqual([
      { value: SYSTEM_DEFAULT_MICROPHONE, label: SYSTEM_DEFAULT_LABEL },
      { value: 'a', label: 'MacBook Pro Microphone' },
      { value: 'b', label: 'Headset' },
    ]);
  });

  /** Chrome reports a "Default" pseudo-device beside the real one. The first
   *  row already means that, and two rows for one choice is a worse list. */
  it('drops the pseudo-devices that mean "follow the system"', () => {
    const rows = microphoneChoices(
      [
        input('default', 'Default - MacBook Pro Microphone'),
        input('communications', 'Communications - Headset'),
        input('real', 'MacBook Pro Microphone'),
      ],
      SYSTEM_DEFAULT_MICROPHONE,
    );
    expect(rows.map((r) => r.value)).toEqual([SYSTEM_DEFAULT_MICROPHONE, 'real']);
  });

  /** Never "Microphone 1". A device the browser will not name is one we cannot
   *  name either, and the picker reports the permission state instead. */
  it('numbers nothing when the browser hides the names', () => {
    const rows = microphoneChoices([input('a', ''), input('b', '')], SYSTEM_DEFAULT_MICROPHONE);
    expect(rows).toEqual([{ value: SYSTEM_DEFAULT_MICROPHONE, label: SYSTEM_DEFAULT_LABEL }]);
  });

  /** An unplugged headset still reads as the choice. Drop it and the picker
   *  claims the system default is selected, which is neither what is stored nor
   *  what the next call asks for. */
  it('keeps a chosen device that is not connected right now', () => {
    const rows = microphoneChoices([input('a', 'MacBook Pro Microphone')], 'headset-gone');
    expect(rows[rows.length - 1]).toEqual({
      value: 'headset-gone',
      label: MISSING_MICROPHONE_LABEL,
    });
  });

  /** "Not connected" is a claim, and a list that named nothing cannot support
   *  it: the device may be plugged in and about to answer the next call. The
   *  row still has to be there, because it is the one that gets the tick. */
  it('keeps the choice without calling it gone when nothing was named', () => {
    for (const devices of [[], [input('a', '')], [output('o', 'Speakers')]]) {
      const rows = microphoneChoices(devices, 'headset');
      expect(rows[rows.length - 1]).toEqual({
        value: 'headset',
        label: UNNAMED_MICROPHONE_LABEL,
      });
    }
  });

  it('does not duplicate a chosen device that is connected', () => {
    const rows = microphoneChoices([input('a', 'Headset')], 'a');
    expect(rows).toEqual([
      { value: SYSTEM_DEFAULT_MICROPHONE, label: SYSTEM_DEFAULT_LABEL },
      { value: 'a', label: 'Headset' },
    ]);
  });
});
