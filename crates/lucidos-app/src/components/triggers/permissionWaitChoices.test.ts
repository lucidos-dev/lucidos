import { describe, expect, it } from 'vitest';
import {
  WORKSPACE_DEFAULT_WAIT,
  permissionWaitFromValue,
  permissionWaitOptions,
  permissionWaitValue,
} from './permissionWaitChoices';

describe('permission wait choices', () => {
  it('round-trips every wire form through the Dropdown value', () => {
    for (const wait of ['deny', 15, 'indefinite'] as const) {
      expect(permissionWaitFromValue(permissionWaitValue(wait))).toBe(wait);
    }
    expect(permissionWaitValue(undefined)).toBe(WORKSPACE_DEFAULT_WAIT);
    expect(permissionWaitFromValue(WORKSPACE_DEFAULT_WAIT)).toBeNull();
  });

  it('keeps a stored wait that no preset matches', () => {
    const values = permissionWaitOptions('45').map(o => o.value);
    expect(values).toEqual(['', 'deny', '5', '15', '30', '45', '60', 'indefinite']);
  });

  it('offers the presets alone when the stored wait is one of them', () => {
    expect(permissionWaitOptions('30').map(o => o.value))
      .toEqual(['', 'deny', '5', '15', '30', '60', 'indefinite']);
  });
});
