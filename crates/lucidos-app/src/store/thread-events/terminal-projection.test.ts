import { describe, it, expect } from 'vitest';
import { terminalProjection, type ThreadMeta } from './thread-meta';

function meta(): ThreadMeta {
  return {} as ThreadMeta;
}

describe('the live terminal projection', () => {
  it('marks a terminal live when it opens and clears it when it exits', () => {
    const m = meta();
    expect(terminalProjection(m, { type: 'TerminalOpened', terminal_id: 'a' }, 10)).toBe(true);
    expect(m.liveTerminal).toEqual({ terminalId: 'a' });
    expect(terminalProjection(m, { type: 'TerminalExited', terminal_id: 'a' }, 11)).toBe(true);
    expect(m.liveTerminal).toBeNull();
  });

  it('ignores an older event loaded after a newer one', () => {
    const m = meta();
    terminalProjection(m, { type: 'TerminalOpened', terminal_id: 'b' }, 20);
    expect(terminalProjection(m, { type: 'TerminalExited', terminal_id: 'a' }, 5)).toBe(false);
    expect(m.liveTerminal?.terminalId).toBe('b');
  });

  it('never resurrects a terminal when its open loads after its exit', () => {
    const m = meta();
    terminalProjection(m, { type: 'TerminalOpened', terminal_id: 'a' }, 10);
    terminalProjection(m, { type: 'TerminalExited', terminal_id: 'a' }, 20);
    expect(terminalProjection(m, { type: 'TerminalOpened', terminal_id: 'a' }, 10)).toBe(false);
    expect(m.liveTerminal).toBeNull();
  });

  it('keeps a newer terminal live when an earlier one exits', () => {
    const m = meta();
    terminalProjection(m, { type: 'TerminalOpened', terminal_id: 'b' }, 20);
    expect(terminalProjection(m, { type: 'TerminalExited', terminal_id: 'a' }, 21)).toBe(false);
    expect(m.liveTerminal?.terminalId).toBe('b');
  });

  it('touches nothing for any other event', () => {
    const m = meta();
    expect(terminalProjection(m, { type: 'MessageReceived' }, 1)).toBe(false);
    expect(m.liveTerminal).toBeUndefined();
  });
});
