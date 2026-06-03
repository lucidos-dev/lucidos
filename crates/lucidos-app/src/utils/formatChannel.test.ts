import { describe, it, expect } from 'vitest';
import { formatChannel } from './formatChannel';

describe('formatChannel', () => {
  it('formats known channels', () => {
    expect(formatChannel('chat')).toBe('Lucidos');
    expect(formatChannel('claude_code')).toBe('Claude Code');
    expect(formatChannel('trigger')).toBe('Trigger');
  });

  it('returns ERROR for error_unknown_channel', () => {
    expect(formatChannel('error_unknown_channel')).toBe('ERROR');
  });

  it('returns raw value for unrecognized channels', () => {
    expect(formatChannel('something_else')).toBe('something_else');
  });
});
