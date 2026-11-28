import { describe, it, expect, vi } from 'vitest';
import { postToFrame } from './app-frame-channel';

describe('postToFrame', () => {
  it('posts to the frame window with the wildcard target, since its origin is opaque', () => {
    const stub = { postMessage: vi.fn() };
    postToFrame(stub as unknown as MessageEventSource, { type: 'lucidos:bridge:push' });
    expect(stub.postMessage).toHaveBeenCalledWith({ type: 'lucidos:bridge:push' }, '*');
  });
});
