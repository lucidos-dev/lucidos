import { describe, it, expect } from 'vitest';
import { wrapHighlight, selectedOptionIndex } from '../ControlOptionList';
import { offeredControlCommands } from '../../chat/CodingAgentControlMenu';

describe('wrapHighlight', () => {
  it('steps down within bounds', () => {
    expect(wrapHighlight(0, 3, 1)).toBe(1);
    expect(wrapHighlight(1, 3, 1)).toBe(2);
  });

  it('wraps from the last row to the first when stepping down', () => {
    expect(wrapHighlight(2, 3, 1)).toBe(0);
  });

  it('steps up within bounds', () => {
    expect(wrapHighlight(2, 3, -1)).toBe(1);
  });

  it('wraps from the first row to the last when stepping up', () => {
    expect(wrapHighlight(0, 3, -1)).toBe(2);
  });

  it('returns 0 for an empty list rather than a negative index', () => {
    expect(wrapHighlight(0, 0, 1)).toBe(0);
    expect(wrapHighlight(0, 0, -1)).toBe(0);
  });
});

describe('selectedOptionIndex', () => {
  const opts = [{ value: 'a' }, { value: 'b' }, { value: 'c' }];

  it('returns the index of the current value so the sub-menu opens on it', () => {
    expect(selectedOptionIndex(opts, 'b')).toBe(1);
  });

  it('falls back to 0 when the current value is absent', () => {
    // A stored model/effort that has since been disabled or removed must still
    // land on a valid row instead of -1 (which would highlight nothing).
    expect(selectedOptionIndex(opts, 'gone')).toBe(0);
  });

  it('falls back to 0 for an empty option list', () => {
    expect(selectedOptionIndex([], 'a')).toBe(0);
  });
});

describe('offeredControlCommands', () => {
  const COMMANDS = [
    { subtype: 'set_model', label: 'Model', params: [] },
    { subtype: 'set_reasoning_effort', label: 'Reasoning Effort', params: [] },
    { subtype: 'set_permission_mode', label: 'Permission Mode', params: [] },
  ];

  it('never offers the effort command on its own', () => {
    // A model selection is one pick, so the tier rides on the `set_model`
    // rows. The command still exists on the wire for the reconciling request.
    expect(offeredControlCommands(COMMANDS).map((c) => c.subtype))
      .toEqual(['set_model', 'set_permission_mode']);
  });
});
