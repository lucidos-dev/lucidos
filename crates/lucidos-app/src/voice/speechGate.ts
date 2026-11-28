
export interface SpeechGateSettings {
  threshold: number;
  framesToOpen: number;
  framesToClose: number;
}

export const SPEECH_GATE_DEFAULTS: SpeechGateSettings = {
  threshold: 0.03,
  framesToOpen: 3,
  framesToClose: 8,
};

export interface SpeechGateState {
  open: boolean;
  run: number;
}

export const SPEECH_GATE_SHUT: SpeechGateState = { open: false, run: 0 };

export function frameEnergy(samples: Float32Array): number {
  if (samples.length === 0) return 0;
  let sum = 0;
  for (let i = 0; i < samples.length; i++) sum += samples[i] * samples[i];
  return Math.sqrt(sum / samples.length);
}

export function stepSpeechGate(
  state: SpeechGateState,
  rms: number,
  settings: SpeechGateSettings = SPEECH_GATE_DEFAULTS,
): SpeechGateState {
  const loud = rms >= settings.threshold;
  if (loud === state.open) return state.run === 0 ? state : { open: state.open, run: 0 };
  const run = state.run + 1;
  const needed = state.open ? settings.framesToClose : settings.framesToOpen;
  if (run < needed) return { open: state.open, run };
  return { open: !state.open, run: 0 };
}
