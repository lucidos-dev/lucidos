/**
 * The wiring between a press, a thread and a call.
 *
 * Two properties matter here and nowhere else. Voice has no entry point of its
 * own, so a call resolves its thread through the same path typing uses. And a
 * call belongs to that thread, so leaving it rings off.
 */
import { describe, it, expect } from 'vitest';
import { signal } from '@preact/signals';
import { NO_THREAD_TO_CALL_ON, createVoiceCallStore, reportCallProblem } from './voice';
import { activeMenuItem, settingsSubview, toasts } from './store';
import { CALL_REFUSED, NO_VOICE_MODEL } from '../voice/refusals';
import type { AudioDevice, CallPorts, CallSocket, SocketHandlers } from '../voice/ports';

const THREAD = 'thread-1';

/** A promise the test settles by hand, to hold a thread row in flight. */
function deferred(): { promise: Promise<void>; resolve: () => void } {
  let settle: () => void = () => undefined;
  const promise = new Promise<void>((done) => {
    settle = done;
  });
  return { promise, resolve: () => settle() };
}

function device(): AudioDevice {
  return { play: () => undefined, stopPlayback: () => undefined, close: () => Promise.resolve() };
}

function fixture(opts: { resolveThread?: () => string; awaitThread?: () => Promise<void> } = {}) {
  const focused = signal<string | null>(THREAD);
  const problems: string[] = [];
  const dialled: string[] = [];
  const sent: string[] = [];
  let handlers: SocketHandlers | null = null;
  let primes = 0;
  let releases = 0;

  const socket: CallSocket = {
    sendText: (text) => sent.push(JSON.parse(text).type as string),
    sendAudio: () => undefined,
    close: () => undefined,
  };

  const ports: CallPorts = {
    prime: () => {
      primes++;
    },
    release: () => {
      releases++;
    },
    openAudio: () => Promise.resolve(device()),
    openSocket: (threadId, socketHandlers) => {
      dialled.push(threadId);
      handlers = socketHandlers;
      return socket;
    },
    probeUpgrade: () => Promise.resolve(true),
  };

  const store = createVoiceCallStore({
    ports,
    focused,
    resolveThread: opts.resolveThread ?? (() => focused.value ?? 'fresh-draft'),
    awaitThread: opts.awaitThread ?? (() => Promise.resolve()),
    onProblem: (message) => problems.push(message),
  });

  async function settle(): Promise<void> {
    for (let i = 0; i < 6; i++) await Promise.resolve();
  }

  async function goLive(): Promise<void> {
    store.press();
    await settle();
    handlers?.onOpen();
    handlers?.onText(
      JSON.stringify({
        type: 'session_started',
        audio: { sample_rate_hz: 24_000, channels: 1, encoding: 'pcm_s16le' },
      }),
    );
  }

  return {
    store,
    focused,
    problems,
    dialled,
    sent,
    settle,
    goLive,
    primes: () => primes,
    releases: () => releases,
    close: () => handlers?.onClose(),
    say: (frame: object) => handlers?.onText(JSON.stringify(frame)),
  };
}

describe('the thread a call runs on', () => {
  it('is the focused one', async () => {
    const f = fixture();
    await f.goLive();
    expect(f.dialled).toEqual([THREAD]);
    expect(f.store.call.value.phase).toBe('listening');
    f.store.dispose();
  });

  it('is a fresh draft when composing, by the path typing uses', async () => {
    // Faithful to `ensureFocusedComposeThread`, which FOCUSES the draft it
    // allocates. The dial's own focus check reads that, so a fake which only
    // returned an id would make this test disagree with the app.
    const f = fixture({
      resolveThread: () => {
        f.focused.value = 'new-draft';
        return 'new-draft';
      },
    });
    f.focused.value = null;
    f.store.press();
    await f.settle();
    expect(f.dialled).toEqual(['new-draft']);
    f.store.dispose();
  });

  it('waits for the thread row before dialling', async () => {
    const row = deferred();
    const f = fixture({ awaitThread: () => row.promise });
    f.store.press();
    await f.settle();
    expect(f.dialled).toEqual([]);
    row.resolve();
    await f.settle();
    expect(f.dialled).toEqual([THREAD]);
    f.store.dispose();
  });

  it('absorbs a second press while the thread row is still landing', async () => {
    const row = deferred();
    const f = fixture({ awaitThread: () => row.promise });
    f.store.press();
    f.store.press();
    row.resolve();
    await f.settle();
    // One call, not a call placed and immediately rung off by its own second
    // press. The toggle still reads as off during that window.
    expect(f.dialled).toEqual([THREAD]);
    expect(f.sent).toEqual([]);
    expect(f.store.call.value.phase).toBe('connecting');
    f.store.dispose();
  });

  it('says so when no thread can be started, and dials nothing', async () => {
    const f = fixture({ awaitThread: () => Promise.reject(new Error('offline')) });
    f.store.press();
    await f.settle();
    expect(f.dialled).toEqual([]);
    expect(f.problems[0]).toContain(NO_THREAD_TO_CALL_ON);
    // The press woke the audio device, and no call took it.
    expect(f.releases()).toBe(1);
    f.store.dispose();
  });

  it('drops a dial whose reader moved on while the row was landing', async () => {
    const row = deferred();
    const f = fixture({ awaitThread: () => row.promise });
    f.store.press();
    // The focus watcher runs here and finds an idle call, so nothing stops the
    // pending dial but the check inside it.
    f.focused.value = 'somewhere-else';
    row.resolve();
    await f.settle();
    expect(f.dialled).toEqual([]);
    expect(f.releases()).toBe(1);
    expect(f.store.call.value.phase).toBe('idle');
    f.store.dispose();
  });

  it('unlocks audio inside the press, before anything is awaited', () => {
    const f = fixture();
    f.store.press();
    expect(f.primes()).toBe(1);
    f.store.dispose();
  });
});

describe('the call belongs to its thread', () => {
  it('rings off when that thread is left', async () => {
    const f = fixture();
    await f.goLive();
    f.focused.value = 'somewhere-else';
    expect(f.sent).toEqual(['hang_up']);
    expect(f.store.call.value.phase).toBe('ending');
    f.store.dispose();
  });

  it('stays up while the focus does not move', async () => {
    const f = fixture();
    await f.goLive();
    f.focused.value = THREAD;
    expect(f.sent).toEqual([]);
    f.store.dispose();
  });

  it('rings off when the focus is dropped entirely', async () => {
    const f = fixture();
    await f.goLive();
    f.focused.value = null;
    expect(f.sent).toEqual(['hang_up']);
    f.store.dispose();
  });

  it('stops watching once disposed', async () => {
    const f = fixture();
    await f.goLive();
    f.store.dispose();
    f.focused.value = 'somewhere-else';
    expect(f.sent).toEqual([]);
  });
});

describe('the toggle', () => {
  it('ends the call rather than placing a second one', async () => {
    const f = fixture();
    await f.goLive();
    f.store.press();
    expect(f.dialled).toEqual([THREAD]);
    expect(f.sent).toEqual(['hang_up']);
    f.store.dispose();
  });
});

/**
 * A reason the reader can act on carries the way to act on it.
 *
 * The engine's talker refusal says to set a model in Settings. Until the Voice
 * section existed there was no such control, so the sentence named a place
 * that was not there. The button is what makes it true.
 */
describe('the way out of a call that could not run', () => {
  it('lands the talker refusal on the settings that fix it', () => {
    toasts.value = [];
    reportCallProblem(NO_VOICE_MODEL);
    const [toast] = toasts.value;
    expect(toast.message).toContain('No voice model');
    expect(toast.action?.label).toBe('Open settings');

    toast.action?.onClick();
    expect(settingsSubview.value).toBe('models');
    expect(activeMenuItem.value).toBe('settings');
  });

  it('offers no button for a reason no setting fixes', () => {
    toasts.value = [];
    reportCallProblem(CALL_REFUSED);
    expect(toasts.value[0].action).toBeUndefined();
  });
});

describe('why a call ended', () => {
  it('is reported once the strip has gone with it', async () => {
    const f = fixture();
    await f.goLive();
    f.say({ type: 'error', message: 'No voice model is configured. Set one in Settings.' });
    expect(f.problems).toEqual([]);
    f.close();
    expect(f.problems).toEqual(['No voice model is configured. Set one in Settings.']);
    f.store.dispose();
  });

  it('says nothing when a call simply ended', async () => {
    const f = fixture();
    await f.goLive();
    f.say({ type: 'session_ended', reason: 'hangup' });
    expect(f.problems).toEqual([]);
    f.store.dispose();
  });
});
