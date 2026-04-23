import { describe, it, expect } from 'vitest';
import { formatChannel, formatThreadRoute } from './formatChannel';

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

describe('formatThreadRoute', () => {
  it('user chat → User → Lucidos', () => {
    expect(formatThreadRoute('user', 'chat')).toBe('User → Lucidos');
  });

  it('user claude_code → User → Claude Code', () => {
    expect(formatThreadRoute('user', 'claude_code')).toBe('User → Claude Code');
  });

  it('system trigger → System → Lucidos', () => {
    expect(formatThreadRoute('system', 'trigger')).toBe('System → Lucidos');
  });

  it('system claude_code → System → Claude Code (CC sub-thread of trigger run)', () => {
    expect(formatThreadRoute('system', 'claude_code')).toBe('System → Claude Code');
  });

  it('system chat → System → Lucidos', () => {
    expect(formatThreadRoute('system', 'chat')).toBe('System → Lucidos');
  });

  it('defaults unknown initiator to User', () => {
    expect(formatThreadRoute('', 'chat')).toBe('User → Lucidos');
  });

  it('maps error_unknown_channel to ERROR', () => {
    expect(formatThreadRoute('user', 'error_unknown_channel')).toBe('User → ERROR');
  });
});
