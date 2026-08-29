import { describe, it, expect } from 'vitest';
import { parseServerFrame } from './frames';

describe('reading a text frame', () => {
  it('reads every frame the engine sends', () => {
    expect(parseServerFrame('{"type":"talker_turn_ended"}')).toEqual({
      type: 'talker_turn_ended',
    });
    expect(parseServerFrame('{"type":"user_turn_ended","transcript":"hello"}')).toEqual({
      type: 'user_turn_ended',
      transcript: 'hello',
    });
  });

  it('ignores a frame this bundle has not learnt', () => {
    expect(parseServerFrame('{"type":"something_newer"}')).toBeNull();
  });

  it('ignores malformed JSON rather than killing the call', () => {
    expect(parseServerFrame('not json')).toBeNull();
    expect(parseServerFrame('')).toBeNull();
  });

  it('ignores JSON that is not a tagged object', () => {
    expect(parseServerFrame('null')).toBeNull();
    expect(parseServerFrame('42')).toBeNull();
    expect(parseServerFrame('["session_started"]')).toBeNull();
    expect(parseServerFrame('{"kind":"session_started"}')).toBeNull();
  });

  it('ignores a known tag whose payload is missing or the wrong shape', () => {
    expect(parseServerFrame('{"type":"talker_transcript"}')).toBeNull();
    expect(parseServerFrame('{"type":"talker_transcript","text":7}')).toBeNull();
    expect(parseServerFrame('{"type":"user_turn_ended","transcript":null}')).toBeNull();
    expect(parseServerFrame('{"type":"error"}')).toBeNull();
    expect(parseServerFrame('{"type":"session_started","audio":"pcm"}')).toBeNull();
  });

  it('reads an empty string as a payload, because the engine may send one', () => {
    expect(parseServerFrame('{"type":"user_turn_ended","transcript":""}')).toEqual({
      type: 'user_turn_ended',
      transcript: '',
    });
  });
});
