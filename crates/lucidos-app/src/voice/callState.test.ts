import { describe, it, expect } from 'vitest';
import type { ServerFrame } from './frames';
import {
  CALL_IDLE,
  type CallEffect,
  type CallInput,
  type CallPhase,
  type CallState,
  HEARING_YOU,
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
/** The local gate opening and shutting on the caller's own voice. */
const SPOKE: CallInput = { kind: 'speech', open: true };
const HUSHED: CallInput = { kind: 'speech', open: false };
const TIMED_OUT: CallInput = { kind: 'utterance-timeout' };
const frame = (f: ServerFrame): CallInput => ({ kind: 'frame', frame: f });
/** The engine reporting the caller's finished words. */
const HEARD: CallInput = frame({ type: 'user_turn_ended', transcript: 'what is on today' });
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

describe('who has the floor', () => {
  it('hands it to the talker on the first delta of a reply', () => {
    const { state } = drive(
      [
        frame({ type: 'talker_transcript', text: 'Two ' }),
        frame({ type: 'talker_transcript', text: 'things.' }),
      ],
      live(),
    );
    expect(state.phase).toBe('speaking');
  });

  it('returns it when the talker finishes', () => {
    const { state } = drive([frame({ type: 'talker_turn_ended' })], talking());
    expect(state.phase).toBe('listening');
  });

  it('leaves a finished caller utterance alone: the floor was already theirs', () => {
    const before = live();
    const { state, effects } = drive(
      [frame({ type: 'user_turn_ended', transcript: 'what is on today' })],
      before,
    );
    expect(state).toEqual(before);
    expect(effects).toEqual([]);
  });

  it('keeps no words of its own, so nothing on screen can go stale', () => {
    const words = ['what is on today', 'one moment'];
    const { state } = drive(
      [
        frame({ type: 'user_turn_ended', transcript: words[0] }),
        frame({ type: 'talker_transcript', text: words[1] }),
      ],
      live(),
    );
    expect(JSON.stringify(state)).not.toContain(words[0]);
    expect(JSON.stringify(state)).not.toContain(words[1]);
  });
});

describe('barge-in', () => {
  it('stops playback and tells the engine, while the talker is speaking', () => {
    const { state, effects } = drive([SPOKE], talking());
    expect(state.phase).toBe('listening');
    expect(sent(effects)).toEqual(['barge_in']);
    expect(has(effects, 'stop-playback')).toBe(true);
  });

  /** The gate opens once per utterance, so the reducer cannot be asked twice
   *  for the same one. The floor it just handed back is the second guard. */
  it('cuts the talker off once, however long the caller keeps talking', () => {
    const { effects } = drive([SPOKE, HUSHED, SPOKE], talking());
    expect(sent(effects)).toEqual(['barge_in']);
  });

  it('says nothing when nobody is on a call', () => {
    expect(drive([SPOKE]).effects).toEqual([]);
  });

  it('stops playback when the engine confirms the cut', () => {
    const { state, effects } = drive([frame({ type: 'interrupted' })], talking());
    expect(state.phase).toBe('listening');
    expect(has(effects, 'stop-playback')).toBe(true);
  });
});

describe('the caller speaking', () => {
  it('opens an utterance on their own floor, and interrupts nothing', () => {
    const { state, effects } = drive([SPOKE], live());
    expect(state.utterance).toBe('live');
    expect(state.utteranceCount).toBe(1);
    expect(effects).toEqual([]);
  });

  /** The barge-in hands the floor straight back, so the caller is using it
   *  before anything reads the state. Waiting would cost the row 120 ms the
   *  gate has already spent. */
  it('opens one over the talker too, in the same step as the barge-in', () => {
    expect(drive([SPOKE], talking()).state.utterance).toBe('live');
  });

  it('waits on the provider once they stop', () => {
    expect(drive([SPOKE, HUSHED], live()).state.utterance).toBe('landing');
  });

  /** The gate shuts on 320 ms of quiet and a provider endpoints on longer. A
   *  breath mid-sentence is still inside the turn the engine will report.
   *  Counted as a second, its row would wait on words that never come. */
  it('reads a pause as one utterance, not two', () => {
    const { state } = drive([SPOKE, HUSHED, SPOKE], live());
    expect(state.utterance).toBe('live');
    expect(state.utteranceCount).toBe(1);
  });

  it('counts a turn the provider ended as a new one', () => {
    const { state } = drive([SPOKE, HUSHED, HEARD, SPOKE], live());
    expect(state.utterance).toBe('live');
    expect(state.utteranceCount).toBe(2);
  });

  it('ignores a gate edge on a call that is not live', () => {
    expect(drive([SPOKE]).state.utterance).toBe('none');
    expect(drive([SPOKE], drive([PRESS]).state).state.utterance).toBe('none');
  });

  /** The talker answering PROVES the provider ended that turn and read it. It
   *  also shuts the gate, and a row left `live` behind a shut gate could never
   *  be retracted: the falling edge that retracts it is spent. */
  it('hands the utterance to the words when the talker takes the floor', () => {
    const speaking = drive([SPOKE], live()).state;
    const { state, effects } = drive([frame({ type: 'talker_transcript', text: 'well' })], speaking);
    expect(state.phase).toBe('speaking');
    expect(state.utterance).toBe('transcribed');
    expect(has(effects, 'forget-speech')).toBe(true);
  });
});

describe('an utterance that never becomes words', () => {
  /** `call.rs` refuses to hold a wordless transcript, so an empty one is the
   *  engine saying the provider heard nothing worth writing down. */
  it('goes at once when the engine reports an empty transcript', () => {
    const { state } = drive(
      [SPOKE, HUSHED, frame({ type: 'user_turn_ended', transcript: '   ' })],
      live(),
    );
    expect(state.utterance).toBe('none');
  });

  it('goes when the bound runs out with nothing said', () => {
    const { state } = drive([SPOKE, HUSHED, TIMED_OUT], live());
    expect(state.utterance).toBe('none');
  });

  it('goes when the words are confirmed and the row never arrives', () => {
    const { state } = drive([SPOKE, HUSHED, HEARD, TIMED_OUT], live());
    expect(state.utterance).toBe('none');
  });

  it('goes with the call, whichever way the call ends', () => {
    const speaking = drive([SPOKE], live()).state;
    expect(drive([PRESS], speaking).state.utterance).toBe('none');
    expect(drive([{ kind: 'leave' }], speaking).state.utterance).toBe('none');
    expect(drive([{ kind: 'socket-closed' }], speaking).state.utterance).toBe('none');
    expect(drive([{ kind: 'failed', message: 'gone' }], speaking).state.utterance).toBe('none');
  });

  it('cannot be timed out while the caller is still talking', () => {
    const speaking = drive([SPOKE], live()).state;
    expect(drive([TIMED_OUT], speaking).state.utterance).toBe('live');
  });
});

describe('an utterance the engine has words for', () => {
  it('waits on the talker deciding what to do with them', () => {
    const { state } = drive([SPOKE, HUSHED, HEARD], live());
    expect(state.utterance).toBe('transcribed');
  });

  /** The gate can miss an utterance said too quietly to clear it. Nothing was
   *  drawn for that one, so there is nothing to keep on screen. */
  it('starts nothing when the gate never heard the caller at all', () => {
    expect(drive([HEARD], live()).state.utterance).toBe('none');
  });

  /** A provider can endpoint mid-breath, so this lands while the caller is
   *  audibly still going. Withdrawing their bubble then is the one thing this
   *  row must never do, whatever the provider made of the noise. */
  it('withdraws no bubble while the caller is still speaking', () => {
    const talking = drive([SPOKE], live()).state;
    const { state } = drive([frame({ type: 'user_turn_ended', transcript: '' })], talking);
    expect(state.utterance).toBe('live');
  });

  /** Words are a different matter: the turn is over and a row is coming, so
   *  whatever they say next is a turn of its own. */
  it('ends the turn on words, even mid-breath', () => {
    const talking = drive([SPOKE], live()).state;
    expect(drive([HEARD], talking).state.utterance).toBe('transcribed');
  });
});

describe('reading a phase', () => {
  const phases: CallPhase[] = ['idle', 'connecting', 'listening', 'speaking', 'ending'];

  it('calls exactly the two live phases live', () => {
    expect(phases.filter(isLive)).toEqual(['listening', 'speaking']);
  });

  it('calls everything but idle a call', () => {
    expect(phases.filter(isOnCall)).toEqual(['connecting', 'listening', 'speaking', 'ending']);
  });

  it('labels every phase, and says nothing when there is no call', () => {
    expect(callStatusLabel(CALL_IDLE)).toBe('');
    for (const phase of phases.filter(isOnCall)) {
      expect(callStatusLabel({ ...CALL_IDLE, phase })).not.toBe('');
    }
  });

  it('says the caller is being heard while they speak', () => {
    expect(callStatusLabel(drive([SPOKE], live()).state)).toBe(HEARING_YOU);
  });

  /** Once they stop, the call is back to plain listening. Announcing each wait
   *  separately would speak three times per utterance. */
  it('goes back to the phase once they stop', () => {
    const waiting = drive([SPOKE, HUSHED], live()).state;
    expect(callStatusLabel(waiting)).toBe(callStatusLabel(live()));
    expect(callStatusLabel(drive([HEARD], waiting).state)).toBe(callStatusLabel(live()));
  });
});
