
export const PLAYBACK_LEAD_SECONDS = 0.2;

export const PLAYBACK_LEAD_MAX_SECONDS = 0.6;

export const QUIET_STREAM_SECONDS = 1;

export interface Playback {
  cursor: number;
  lead: number;
  gaps: number;
  silentSeconds: number;
}

export const PLAYBACK_START: Playback = {
  cursor: 0,
  lead: PLAYBACK_LEAD_SECONDS,
  gaps: 0,
  silentSeconds: 0,
};

export interface Placed {
  startAt: number;
  playback: Playback;
}

export function placeChunk(state: Playback, now: number, durationSeconds: number): Placed {
  const drained = now - state.cursor;
  const ranDry = state.cursor > 0 && drained > 0 && drained <= QUIET_STREAM_SECONDS;
  const lead = ranDry
    ? Math.min(PLAYBACK_LEAD_MAX_SECONDS, Math.max(state.lead, drained + PLAYBACK_LEAD_SECONDS))
    : state.lead;
  const startAt = state.cursor > now ? state.cursor : now + lead;
  return {
    startAt,
    playback: {
      cursor: startAt + durationSeconds,
      lead,
      gaps: ranDry ? state.gaps + 1 : state.gaps,
      silentSeconds: ranDry
        ? state.silentSeconds + (startAt - state.cursor)
        : state.silentSeconds,
    },
  };
}

export function restartPlayback(state: Playback): Playback {
  return { ...state, cursor: 0 };
}
