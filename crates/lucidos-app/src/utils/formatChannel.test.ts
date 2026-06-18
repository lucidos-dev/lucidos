import { describe, it, expect } from 'vitest';
import { formatChannel, formatThreadChannelLabel } from './formatChannel';

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

describe('formatThreadChannelLabel', () => {
  it('reads "Codex" for a Codex-backed coding-agent thread', () => {
    expect(formatThreadChannelLabel('claude_code', 'codex')).toBe('Codex');
  });

  it('reads "Claude Code" for a Claude Code coding-agent thread', () => {
    expect(formatThreadChannelLabel('claude_code', 'claude-code')).toBe('Claude Code');
  });

  it('defaults a coding-agent thread with no backend to Claude Code', () => {
    expect(formatThreadChannelLabel('claude_code')).toBe('Claude Code');
    expect(formatThreadChannelLabel('claude_code', null)).toBe('Claude Code');
  });

  it('renders no tag for plain chat / Lucidos threads', () => {
    // A regular Lucidos thread carries no channel chip — the absence IS the
    // signal. `formatChannel` still names the channel for the filter dropdown.
    expect(formatThreadChannelLabel('chat')).toBe('');
    expect(formatThreadChannelLabel('lucidos')).toBe('');
  });

  it('ignores codingAgent for non-coding channels', () => {
    // A stray backend on a non-CC channel must never relabel it: chat stays
    // tag-less, trigger stays "Trigger".
    expect(formatThreadChannelLabel('chat', 'codex')).toBe('');
    expect(formatThreadChannelLabel('trigger', 'codex')).toBe('Trigger');
  });
});
