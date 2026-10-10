import { describe, it, expect } from 'vitest';
import {
  PERMISSION_MODE_OPTIONS,
  isPermissionMode,
} from './CodingAgentPermissionSection';
import { CC_PERMISSION_MODES } from '../../store/actions/preferences';
import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';

describe('coding agent permission mode', () => {
  it('puts the safe default first', () => {
    // Leading with Auto would invite a click-through into the classifier
    // without reading what it costs.
    expect(PERMISSION_MODE_OPTIONS[0].value)
      .toBe(PREFERENCE_CATALOG.coding_agent_claude_permission_mode.fallback);
  });

  it('gives every option a label and a trade-off line', () => {
    for (const option of PERMISSION_MODE_OPTIONS) {
      expect(option.label.length).toBeGreaterThan(0);
      expect(option.description?.length ?? 0).toBeGreaterThan(0);
    }
  });

  it('refuses a value outside the accepted set', () => {
    for (const mode of CC_PERMISSION_MODES) expect(isPermissionMode(mode)).toBe(true);
    for (const rejected of ['', 'acceptEdits', 'default', 'bypassPermissions', 'plan']) {
      expect(isPermissionMode(rejected)).toBe(false);
    }
  });
});
