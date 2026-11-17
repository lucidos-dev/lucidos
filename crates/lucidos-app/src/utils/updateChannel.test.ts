import { expect, it } from 'vitest';
import { withUpdateChannel } from './updateChannel';
it('adds a single channel identity to release and client versions', () => {
  expect(withUpdateChannel('9.1.0', 'beta')).toBe('9.1.0 · Beta');
  expect(withUpdateChannel('9.1.0', 'stable')).toBe('9.1.0 · Stable');
  expect(withUpdateChannel('dev', 'local')).toBe('dev · Local');
  expect(withUpdateChannel('9.1.0', 'custom:staging')).toBe('9.1.0 · Custom: staging');
  expect(withUpdateChannel('dev')).toBe('dev');
});
