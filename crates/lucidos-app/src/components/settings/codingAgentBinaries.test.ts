import { describe, it, expect } from 'vitest';
import {
  binaryStatusLine,
  binaryVersionLabel,
  storedOverride,
} from './CodingAgentBinariesSection';
import type { AgentBinaryStatus } from '../../api/types';

const status = (partial: Partial<AgentBinaryStatus>): AgentBinaryStatus => ({
  path: null,
  source: 'not-found',
  valid: false,
  ...partial,
});

describe('binaryStatusLine', () => {
  it('names the resolution source without repeating the path', () => {
    // The path is shown once, in the input below the line. A row that printed
    // it in both places wrapped the long string twice on a phone.
    expect(
      binaryStatusLine(status({ source: 'override', valid: true, path: '/x/claude' }), 'claude'),
    ).toBe('Configured');
    expect(
      binaryStatusLine(status({ source: 'detected', valid: true, path: '/y/claude' }), 'claude'),
    ).toBe('Auto-detected');
    expect(
      binaryStatusLine(status({ source: 'path', valid: true, path: '/z/codex' }), 'codex'),
    ).toBe('Found on PATH');
    expect(binaryStatusLine(status({}), 'codex')).toContain('install codex');
  });

  it('surfaces the spawn error for an invalid override', () => {
    // The engine's validation message names the preference, and the UI must
    // show it verbatim rather than a generic line (no-hidden-errors). This is
    // the one case that keeps a path in the line, because the path is what is
    // wrong.
    const line = binaryStatusLine(
      status({
        source: 'override',
        valid: false,
        path: '/typo/claude',
        error: "configured Claude Code binary '/typo/claude' does not exist, fix 'coding_agent_claude_path'",
      }),
      'claude',
    );
    expect(line).toContain('coding_agent_claude_path');
  });
});

describe('binaryVersionLabel', () => {
  it('renders the engine-reported version as a v-prefixed token', () => {
    expect(
      binaryVersionLabel(status({ source: 'detected', valid: true, version: '2.1.224' })),
    ).toBe('v2.1.224');
  });

  it('renders nothing when no version is known', () => {
    // An unknown version is an absence, never "unknown" or "v?". The engine
    // omits the field when the probe found nothing recognizable.
    expect(binaryVersionLabel(status({ source: 'detected', valid: true }))).toBe('');
    expect(binaryVersionLabel(status({}))).toBe('');
  });
});

describe('storedOverride', () => {
  it('returns the override path only when one is stored', () => {
    expect(storedOverride(status({ source: 'override', path: '/x/claude' }))).toBe('/x/claude');
    expect(storedOverride(status({ source: 'detected', path: '/y/claude' }))).toBe('');
    expect(storedOverride(status({ source: 'not-found' }))).toBe('');
  });
});
