import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { ui } from './ui';

describe('lucidos.ui.startThread', () => {
  beforeEach(() => {
    vi.stubGlobal('fetch', vi.fn(async () => new Response(null, { status: 200 })));
  });

  afterEach(() => {
    vi.unstubAllGlobals();
  });

  it('POSTs to /api/v1/ui/navigate with target=new-chat (no prompt)', async () => {
    await ui.startThread();
    expect(fetch).toHaveBeenCalledTimes(1);
    const [path, init] = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    expect(path).toBe('/api/v1/ui/navigate');
    expect(init.method).toBe('POST');
    const body = JSON.parse(init.body);
    expect(body.target).toBe('new-chat');
    expect(body.params).toEqual({});
  });

  it('passes prompt through params when provided', async () => {
    await ui.startThread({ prompt: 'Set up a daily standup trigger' });
    const [, init] = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.target).toBe('new-chat');
    expect(body.params).toEqual({ prompt: 'Set up a daily standup trigger' });
  });

  it('omits params.prompt when prompt is empty string', async () => {
    await ui.startThread({ prompt: '' });
    const [, init] = (fetch as unknown as ReturnType<typeof vi.fn>).mock.calls[0];
    const body = JSON.parse(init.body);
    expect(body.params).toEqual({});
  });

  it('rejects when prompt is not a string', async () => {
    await expect(ui.startThread({ prompt: 123 as unknown as string })).rejects.toThrow(TypeError);
    expect(fetch).not.toHaveBeenCalled();
  });
});
