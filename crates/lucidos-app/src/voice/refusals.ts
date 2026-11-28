import { errorDetail } from '../utils/errorDetail';

export class CallSetupError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'CallSetupError';
  }
}

export const UNEXPECTED_SETUP_FAILURE = 'The call could not be started. Try again in a moment.';

export const NO_WEB_AUDIO = 'This browser cannot play audio, so it cannot hold a call.';

export const NO_MICROPHONE_API =
  'This page cannot open a microphone. A call needs a secure connection.';

export function wrongAudioRate(actualHz: number, wantedHz: number): string {
  return `This browser opened audio at ${actualHz} Hz, and a call needs ${wantedHz} Hz. `
    + 'Your voice would not come through, so the call was not placed.';
}

export const CALL_REFUSED = 'Could not connect the call. This thread may already be on one.';

export const NO_ROUTE_FOR_A_CALL =
  'Could not reach Lucidos to place the call. The engine may be down, or the app serving this workspace may need updating.';

export const NO_VOICE_MODEL = 'No voice model is configured. Set one in Settings.';

export function isSettingsProblem(message: string): boolean {
  return message === NO_VOICE_MODEL;
}

export function refusalName(err: unknown): string {
  const name = (err as { name?: unknown } | null)?.name;
  return typeof name === 'string' ? name : '';
}

export function microphoneRefusal(err: unknown): string {
  switch (refusalName(err)) {
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

export function setupRefusal(err: unknown): string {
  return err instanceof CallSetupError ? err.message : UNEXPECTED_SETUP_FAILURE;
}
