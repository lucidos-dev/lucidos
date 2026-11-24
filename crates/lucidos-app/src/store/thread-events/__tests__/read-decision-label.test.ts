import { describe, expect, it } from 'vitest';
import { describeEngineTool } from '../tool-descriptions';

describe('describeEngineTool: request_read', () => {
  it('labels a no quietly', () => {
    expect(describeEngineTool('request_read', { read: false })).toBe('No read needed');
  });

  it('labels a yes, and an older argument-less call, as a request', () => {
    expect(describeEngineTool('request_read', { read: true })).toBe('Asking you to read this reply');
    expect(describeEngineTool('request_read', {})).toBe('Asking you to read this reply');
  });
});
