import { describe, it, expect } from 'vitest';
import {
  PERMISSION_MODE_OPTIONS,
  isPermissionMode,
} from './CodingAgentPermissionSection';
import { CC_PERMISSION_MODES } from '../../store/actions/preferences';

describe('coding agent permission mode', () => {
  it('offers exactly the values the engine accepts', () => {
    // A third option here would save a value the engine rejects, and the
    // toast would be the only sign.
    expect(PERMISSION_MODE_OPTIONS.map((o) => o.value)).toEqual([...CC_PERMISSION_MODES]);
  });

  it('puts the safe default first', () => {
    // The engine's default. Leading with Auto would invite a click-through
    // into the classifier without reading what it costs.
    expect(PERMISSION_MODE_OPTIONS[0].value).toBe('accept-edits');
  });

  it('gives every option a label and a trade-off line', () => {
    for (const option of PERMISSION_MODE_OPTIONS) {
      expect(option.label.length).toBeGreaterThan(0);
      expect(option.description?.length ?? 0).toBeGreaterThan(0);
    }
  });

  it('refuses a value outside the accepted set', () => {
    expect(isPermissionMode('accept-edits')).toBe(true);
    expect(isPermissionMode('auto')).toBe(true);
    for (const rejected of ['', 'acceptEdits', 'default', 'bypassPermissions', 'plan']) {
      expect(isPermissionMode(rejected)).toBe(false);
    }
  });
});
