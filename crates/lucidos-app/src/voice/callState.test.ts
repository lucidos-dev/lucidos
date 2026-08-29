import { describe, it, expect } from 'vitest';
import type { ServerFrame } from './frames';
import {
  CALL_IDLE,
  type CallEffect,
  type CallInput,
  type CallPhase,
  type CallState,
  callStatusLabel,
  isLive,
  isOnCall,
  stepCall,
} from './callState';

const THREAD = 'thread-1';

/** Drive a run of inputs and keep the last state plus every effect. */
function drive(inputs: CallInput[], from: CallState = CALL_IDLE) {
  let state = from;
  const effects: CallEffect[] = [];
  for (const input of inputs) {
    const step = stepCall(state, input);
    state = step.state;
    effects.push(...step.effects);
  }
  return { state, effects };
}

const PRESS: CallInput = { kind: 'toggle', threadId: THREAD };
const frame = (f: ServerFrame): CallInput => ({ kind: 'frame', frame: f });
const STARTED = frame({
  type: 'session_started',
  audio: { sample_rate_hz: 24_000, channels: 1, encoding: 'pcm_s16le' },
});

/** A call that is up and listening. */
function live(): CallState {
  return drive([PRESS, STARTED]).state;
}

/** A call with the talker mid-reply. */
function talking(): CallState {
  return drive([frame({ type: 'talker_transcript', text: 'one moment' })], live()).state;
}

function has(effects: CallEffect[], kind: CallEffect['kind']): boolean {
  return effects.some((e) => e.kind === kind);
}

function sent(effects: CallEffect[]): string[] {
  return effects.flatMap((e) => (e.kind === 'send' ? [e.control.type] : []));
}

describe('placing a call', () => {
  it('opens a socket on the pressed thread', () => {
    const { state, effects } = drive([PRESS]);
    expect(state.phase).toBe('connecting');
    expect(state.threadId).toBe(THREAD);
    expect(effects).toEqual([{ kind: 'open', threadId: THREAD }]);
  });

  it('is listening once the engine says the call is up', () => {
    expect(live().phase).toBe('listening');
  });

  it('clears the note left by a previous failure', () => {
    const failed = drive([PRESS, { kind: 'failed', message: 'No microphone' }]).state;
    expect(failed.note).toBe('No microphone');
    expect(drive([PRESS], failed).state.note).toBeNull();
  });

  it('opens exactly one socket however many times the frame is repeated', () => {
    const { effects } = drive([PRESS, STARTED, STARTED]);
    expect(effects.filter((e) => e.kind === 'open')).toHaveLength(1);
  });
});

describe('the toggle ends what it started', () => {
  it('rings off from a live call', () => {
    const { state, effects } = drive([PRESS], live());
    expect(state.phase).toBe('ending');
    expect(sent(effects)).toEqual(['hang_up']);
    expect(has(effects, 'stop-playback')).toBe(true);
  });

  it('rings off while the talker is mid-reply', () => {
    const { state, effects } = drive([PRESS], talking());
    expect(state.phase).toBe('ending');
    expect(sent(effects)).toEqual(['hang_up']);
  });

  it('drops a call that never connected, with nothing to say goodbye on', () => {
    const { state, effects } = drive([PRESS, PRESS]);
    expect(state).toEqual(CALL_IDLE);
    expect(sent(effects)).toEqual([]);
    expect(has(effects, 'teardown')).toBe(true);
  });

  it('does nothing a second time while a call is already ending', () => {
    const ending = drive([PRESS], live()).state;
    const { state, effects } = drive([PRESS], ending);
    expect(state).toEqual(ending);
    expect(effects).toEqual([]);
  });

  it('never places a second call, whatever thread the press names', () => {
    const { effects } = drive([{ kind: 'toggle', threadId: 'somewhere-else' }], live());
    expect(has(effects, 'open')).toBe(false);
  });
});

describe('every terminal path reaches idle exactly once', () => {
  const paths: [string, CallInput[]][] = [
    ['a second press', [PRESS, frame({ type: 'session_ended', reason: 'hangup' })]],
    ['leaving the thread', [{ kind: 'leave' }, frame({ type: 'session_ended', reason: 'hangup' })]],
    ['the socket dying', [{ kind: 'socket-closed' }]],
    ['the engine ending the session', [frame({ type: 'session_ended', reason: 'disconnected' })]],
    ['a local failure', [{ kind: 'failed', message: 'The microphone was refused.' }]],
    ['a refused handshake', [{ kind: 'refused' }]],
    [
      'a refusal then a close',
      [frame({ type: 'error', message: 'No voice model is configured.' }), { kind: 'socket-closed' }],
    ],
  ];

  for (const [name, inputs] of paths) {
    it(`tears down after ${name}`, () => {
      const { state, effects } = drive(inputs, live());
      expect(state.phase).toBe('idle');
      expect(effects.filter((e) => e.kind === 'teardown')).toHaveLength(1);
    });
  }

  it('ignores a close that arrives when no call is up', () => {
    const { state, effects } = drive([{ kind: 'socket-closed' }]);
    expect(state).toEqual(CALL_IDLE);
    expect(effects).toEqual([]);
  });
});

describe('the reason a call could not run', () => {
  it('survives the teardown, because it is the only explanation left', () => {
    const { state } = drive(
      [
        frame({ type: 'error', message: 'No voice model is configured. Set one in Settings.' }),
        { kind: 'socket-closed' },
      ],
      live(),
    );
    expect(state.phase).toBe('idle');
    expect(state.note).toBe('No voice model is configured. Set one in Settings.');
  });

  it('does not end a call on its own, because the engine carries on too', () => {
    const { state, effects } = drive(
      [frame({ type: 'error', message: 'That control was not one this call understands.' })],
      live(),
    );
    expect(state.phase).toBe('listening');
    expect(effects).toEqual([]);
  });

  /** A refusal carries none, on purpose: the shell is still measuring why, and
   *  reports it once it knows. A guess here would be shown and then contradicted. */
  it('is left empty by a refused handshake, which is still being measured', () => {
    const { state } = drive([{ kind: 'refused' }], live());
    expect(state.phase).toBe('idle');
    expect(state.note).toBeNull();
  });
});

describe('leaving the thread', () => {
  it('ends a live call', () => {
    const { state, effects } = drive([{ kind: 'leave' }], live());
    expect(state.phase).toBe('ending');
    expect(sent(effects)).toEqual(['hang_up']);
  });

  it('does nothing when no call is up', () => {
    expect(drive([{ kind: 'leave' }]).effects).toEqual([]);
  });
});

describe('what the caller and the talker said', () => {
  it('shows the engine transcript of a finished utterance', () => {
    const { state } = drive([frame({ type: 'user_turn_ended', transcript: 'what is on today' })], live());
    expect(state.heard).toBe('what is on today');
  });

  it('accumulates the talker reply from its deltas', () => {
    const { state } = drive(
      [
        frame({ type: 'talker_transcript', text: 'Two ' }),
        frame({ type: 'talker_transcript', text: 'things.' }),
      ],
      live(),
    );
    expect(state.said).toBe('Two things.');
    expect(state.phase).toBe('speaking');
  });

  it('clears the last reply when the next utterance lands', () => {
    const { state } = drive([frame({ type: 'user_turn_ended', transcript: 'and after that' })], talking());
    expect(state.said).toBe('');
    expect(state.heard).toBe('and after that');
  });

  it('returns the floor when the talker finishes', () => {
    const { state } = drive([frame({ type: 'talker_turn_ended' })], talking());
    expect(state.phase).toBe('listening');
  });
});

describe('barge-in', () => {
  it('stops playback and tells the engine, while the talker is speaking', () => {
    const { state, effects } = drive([{ kind: 'barge-in' }], talking());
    expect(state.phase).toBe('listening');
    expect(sent(effects)).toEqual(['barge_in']);
    expect(has(effects, 'stop-playback')).toBe(true);
  });

  it('says nothing when the talker is not speaking', () => {
    expect(drive([{ kind: 'barge-in' }], live()).effects).toEqual([]);
    expect(drive([{ kind: 'barge-in' }]).effects).toEqual([]);
  });

  it('keeps the words the caller already heard', () => {
    const { state } = drive([{ kind: 'barge-in' }], talking());
    expect(state.said).toBe('one moment');
  });

  it('stops playback when the engine confirms the cut', () => {
    const { state, effects } = drive([frame({ type: 'interrupted' })], talking());
    expect(state.phase).toBe('listening');
    expect(has(effects, 'stop-playback')).toBe(true);
  });
});

describe('reading a phase', () => {
  const phases: CallPhase[] = ['idle', 'connecting', 'listening', 'speaking', 'ending'];

  it('calls exactly the two live phases live', () => {
    expect(phases.filter(isLive)).toEqual(['listening', 'speaking']);
  });

  it('shows the strip for everything but idle', () => {
    expect(phases.filter(isOnCall)).toEqual(['connecting', 'listening', 'speaking', 'ending']);
  });

  it('labels every phase, and says nothing when there is no call', () => {
    expect(callStatusLabel('idle')).toBe('');
    for (const phase of phases.filter(isOnCall)) {
      expect(callStatusLabel(phase)).not.toBe('');
    }
  });
});
