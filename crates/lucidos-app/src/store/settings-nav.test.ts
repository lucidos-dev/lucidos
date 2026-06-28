import { describe, it, expect } from 'vitest';
import { SETTINGS_NAV_ITEMS, SETTINGS_SYSTEM_SUBPANEL_ITEMS, settingsSubviewLabel } from './store';

describe('settings navigation', () => {
  it('keeps System last in the main settings list and owns Thread Queue, Backup, Memory, Disk Usage, Environment Variables, and Debugging as subpanels', () => {
    const keys = SETTINGS_NAV_ITEMS.map((item) => item.key);

    expect(keys[keys.length - 1]).toBe('system');
    expect(keys).not.toContain('thread-queue');
    expect(keys).not.toContain('backup');
    expect(keys).not.toContain('memory');
    expect(keys).not.toContain('disk-usage');
    expect(keys).not.toContain('environment-variables');
    expect(keys).not.toContain('debugging');
    expect(SETTINGS_SYSTEM_SUBPANEL_ITEMS.map((item) => item.key)).toEqual(['thread-queue', 'backup', 'memory', 'disk-usage', 'environment-variables', 'debugging']);
    expect(settingsSubviewLabel('thread-queue')).toBe('Thread Queue');
    expect(settingsSubviewLabel('backup')).toBe('Backup');
    expect(settingsSubviewLabel('environment-variables')).toBe('Environment Variables');
    expect(settingsSubviewLabel('debugging')).toBe('Debugging');
  });
});
