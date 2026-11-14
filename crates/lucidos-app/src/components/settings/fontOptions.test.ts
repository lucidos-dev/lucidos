import { describe, it, expect } from 'vitest';
import { FOLLOW_THEME, sanitizeWorkspaceFont, type WorkspaceFont } from '@lucidos/appearance';
import { fontOptions } from './fontOptions';

const brand = sanitizeWorkspaceFont({
  id: 'ws-brand',
  label: 'Brand Sans',
  group: 'sans',
  faces: [{ path: 'fonts/brand/a.woff2', weight: '400', style: 'normal' }],
}) as WorkspaceFont;

describe('fontOptions', () => {
  it('lists no workspace group when there are no workspace fonts', () => {
    const options = fontOptions([]);
    expect(options[0].value).toBe(FOLLOW_THEME);
    expect(options.some(o => o.value === 'group:workspace')).toBe(false);
  });

  it('lists workspace fonts last, under a header that can never be the value', () => {
    const options = fontOptions([brand]);
    const header = options.findIndex(o => o.value === 'group:workspace');
    expect(options[header].disabled).toBe(true);
    expect(options.slice(header + 1)).toEqual([{ value: 'ws-brand', label: 'Brand Sans' }]);
  });

  it('keeps a row for a picked workspace font that is no longer installed', () => {
    const options = fontOptions([brand], 'ws-gone');
    expect(options[options.length - 1]).toEqual({ value: 'ws-gone', label: 'ws-gone (not installed)' });
    expect(fontOptions([brand], 'ws-brand').some(o => o.label.includes('not installed'))).toBe(false);
    expect(fontOptions([], 'inter').some(o => o.value === 'group:workspace')).toBe(false);
  });
});
