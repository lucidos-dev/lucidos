import { expect, it } from 'vitest';
import { remoteSwitcherRows } from './RemoteSwitcherRows';
import { vnodeToText } from '../chat/__tests__/vnodeToText';

it('marks dev@cloud current without marking the local namesake', () => {
  const result = remoteSwitcherRows([{ gateway: 'cloud', workspaces: [{ id: 'dev', name: 'dev', health: 'healthy' }], unreachable: null }], 'dev@cloud', () => {});
  const text = vnodeToText(result);
  expect(text).toContain('@cloud');
  expect(text).toContain('remote');
  const section = result.props.children[0];
  const row = section.props.children[2][0];
  expect(row.props['aria-current']).toBe('page');
  expect(row.props.href).toBeUndefined();
});
it('opens only via the local relay route', () => {
  const result = remoteSwitcherRows([{ gateway: 'cloud', workspaces: [{ id: 'dev', name: 'dev', health: 'healthy' }], unreachable: null }], 'dev', () => {});
  expect(result.props.children[0].props.children[2][0].props.href).toBe('/dev@cloud/');
});
