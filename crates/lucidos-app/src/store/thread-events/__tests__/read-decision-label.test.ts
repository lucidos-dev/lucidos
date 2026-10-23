import { describe, expect, it } from 'vitest';
import { describeEngineTool } from '../exchange';

// The read decision (ADR 0417) runs on every turn, so its no must read
// quietly. A yes still says it asks the user to read the reply.
describe('describeEngineTool: request_read', () => {
  it('labels a no quietly', () => {
    expect(describeEngineTool('request_read', { read: false })).toBe('No read needed');
  });

  it('labels a yes, and an older argument-less call, as a request', () => {
    expect(describeEngineTool('request_read', { read: true })).toBe('Asking you to read this reply');
    expect(describeEngineTool('request_read', {})).toBe('Asking you to read this reply');
  });
});
