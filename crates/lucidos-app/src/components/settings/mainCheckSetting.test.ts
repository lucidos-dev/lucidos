import { describe, expect, it } from 'vitest';
import { PREFERENCE_CATALOG } from '@lucidos/preference-catalog';
import { mainCheckSettingOptions, shownMainCheckSetting } from './MainCheckSettingSection';

describe('main-check setting picker', () => {
  it('offers exactly the catalog values', () => {
    expect(mainCheckSettingOptions().map((o) => o.value)).toEqual([...PREFERENCE_CATALOG.main_check_setting.values]);
  });

  it('shows the stored value, else the engine default, else nothing yet', () => {
    expect(shownMainCheckSetting('another-host', 'here')).toBe('another-host');
    expect(shownMainCheckSetting(null, 'here')).toBe('here');
    expect(shownMainCheckSetting(null, undefined)).toBeNull();
  });
});
