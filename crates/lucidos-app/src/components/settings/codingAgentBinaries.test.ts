import { describe, it, expect } from 'vitest';
import { binaryStatusLine, storedOverride } from './CodingAgentBinariesSection';
import type { AgentBinaryStatus } from '../../api/types';

const status = (partial: Partial<AgentBinaryStatus>): AgentBinaryStatus => ({
  path: null,
  source: 'not-found',
  valid: false,
  ...partial,
});

describe('binaryStatusLine', () => {
  it('describes each resolution source', () => {
    expect(
      binaryStatusLine(status({ source: 'override', valid: true, path: '/x/claude' }), 'claude'),
    ).toContain('/x/claude');
    expect(
      binaryStatusLine(status({ source: 'detected', valid: true, path: '/y/claude' }), 'claude'),
    ).toBe('Auto-detected at /y/claude');
    expect(
      binaryStatusLine(status({ source: 'path', valid: true, path: '/z/codex' }), 'codex'),
    ).toBe('Found on PATH at /z/codex');
    expect(binaryStatusLine(status({}), 'codex')).toContain('install codex');
  });

  it('surfaces the spawn error for an invalid override', () => {
    // The engine's validation message names the preference — the UI must show
    // it verbatim, not a generic line (no-hidden-errors).
    const line = binaryStatusLine(
      status({
        source: 'override',
        valid: false,
        path: '/typo/claude',
        error: "configured Claude Code binary '/typo/claude' does not exist — fix 'coding_agent_claude_path'",
      }),
      'claude',
    );
    expect(line).toContain('coding_agent_claude_path');
  });
});

describe('storedOverride', () => {
  it('returns the override path only when one is stored', () => {
    expect(storedOverride(status({ source: 'override', path: '/x/claude' }))).toBe('/x/claude');
    expect(storedOverride(status({ source: 'detected', path: '/y/claude' }))).toBe('');
    expect(storedOverride(status({ source: 'not-found' }))).toBe('');
  });
});
