/**
 * What the reader is told when a call cannot start.
 *
 * Plain English, always. The engine's own `error` frame forbids a status code
 * and a provider name, and a refusal raised on this side has the same duty: the
 * person who pressed the button is the only reader.
 *
 * Pure, so every message is a test rather than a string nobody ever sees.
 */
import { errorDetail } from '../utils/errorDetail';

/**
 * A refusal whose message is already fit to show.
 *
 * The runner prints a `CallSetupError` as it stands, and replaces anything else
 * with {@link UNEXPECTED_SETUP_FAILURE}. So a stray `TypeError` from a browser
 * internal cannot reach the strip as jargon.
 */
export class CallSetupError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CallSetupError';
  }
}

export const UNEXPECTED_SETUP_FAILURE = 'The call could not be started. Try again in a moment.';

export const NO_WEB_AUDIO = 'This browser cannot play audio, so it cannot hold a call.';

/** No `getUserMedia` at all, which on the web means an insecure origin. */
export const NO_MICROPHONE_API =
  'This page cannot open a microphone. A call needs a secure connection.';

/**
 * The handshake was refused, and a browser never says why.
 *
 * A `WebSocket` hides the response status, so the cause is MEASURED rather
 * than guessed at: the runner asks the engine's echo whether an upgrade
 * survives the hops, and picks one of the two messages below.
 *
 * Guessing is what this replaced, and it misdiagnosed the first real call
 * anybody placed. The old single message named a busy thread and an
 * unreachable engine. The actual cause was neither.
 */
export const CALL_REFUSED = 'Could not connect the call. This thread may already be on one.';

/**
 * The echo did not upgrade either, so the fault is under the call, not in it.
 *
 * Both causes are named because the browser cannot tell them apart. A dead
 * engine and a hop that drops upgrades look identical from here: the socket
 * fails to open and the response is hidden either way.
 *
 * What the echo DOES rule out is the thread. A busy thread refuses the call
 * and leaves the echo alone, so {@link CALL_REFUSED} is the wrong answer here
 * and this one never mentions it.
 */
export const NO_ROUTE_FOR_A_CALL =
  'Could not reach Lucidos to place the call. The engine may be down, or the app serving this workspace may need updating.';

/**
 * The engine's one word for every talker it could not resolve.
 *
 * Mirrored from `api/voice.rs`, which sends this whenever `provider_for`
 * fails: no model, the provider switched off, or no key. All three are
 * settings, which is why one sentence serves them and why it earns a way
 * there. Pinned by `frames.mirror.test.ts`, so a reworded engine does not
 * silently take the link away.
 */
export const NO_VOICE_MODEL = 'No voice model is configured. Set one in Settings.';

/** Does this reason have somewhere for the reader to go and fix it? */
export function isSettingsProblem(message: string): boolean {
  return message === NO_VOICE_MODEL;
}

/** Why the microphone would not open, from what the browser threw. */
export function microphoneRefusal(err: unknown): string {
  const name = err instanceof DOMException ? err.name : '';
  switch (name) {
    case 'NotAllowedError':
    case 'SecurityError':
      return 'Lucidos needs permission to use the microphone. Allow it and try again.';
    case 'NotFoundError':
    case 'OverconstrainedError':
      return 'No microphone was found on this device.';
    case 'NotReadableError':
      return 'The microphone is busy. Close whatever else is using it, then try again.';
    default:
      return `The microphone could not be opened: ${errorDetail(err)}`;
  }
}

/** The message for anything thrown while setting a call up. */
export function setupRefusal(err: unknown): string {
  return err instanceof CallSetupError ? err.message : UNEXPECTED_SETUP_FAILURE;
}
