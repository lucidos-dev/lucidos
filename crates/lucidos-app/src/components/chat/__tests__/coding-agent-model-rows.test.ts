import { describe, it, expect } from 'vitest';
import { pickerRowValue } from '../useRungState';
import type { CodingAgentCommandOption } from '../../../api/client';

const row = (value: string, resolved_model?: string, description = ''): CodingAgentCommandOption => ({
  value, label: value, description, resolved_model,
});

/** The picker as the engine serves it: sorted, and Opus 5.5 folded to the
 *  `opus` alias although Claude Code also lists `claude-opus-5-5[1m]`. */
const served = [
  row('default', 'claude-opus-5-5[1m]'),
  row('claude-fable-5-1', 'claude-fable-5-1'),
  row('opus', 'claude-opus-5-5[1m]'),
  row('sonnet', 'claude-sonnet-5[1m]'),
  row('haiku', 'claude-haiku-4-5-20251001'),
];

describe('pickerRowValue', () => {
  it('keeps a model that is a row of its own', () => {
    expect(pickerRowValue(served, 'sonnet')).toBe('sonnet');
    expect(pickerRowValue(served, 'default')).toBe('default');
  });

  it('marks the row that runs the id a thread recorded', () => {
    expect(pickerRowValue(served, 'claude-sonnet-5[1m]')).toBe('sonnet');
  });

  it('marks the row a folded id runs as, never Default', () => {
    expect(pickerRowValue(served, 'claude-opus-5-5[1m]')).toBe('opus');
  });

  it('leaves a model no row runs as it is, for the picker to name', () => {
    expect(pickerRowValue(served, 'claude-opus-4-1')).toBe('claude-opus-4-1');
  });
});
