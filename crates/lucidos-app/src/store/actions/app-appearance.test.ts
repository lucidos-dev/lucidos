// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { APPEARANCE_CHANNEL } from '@lucidos/appearance';
import { installAppAppearanceSync, pushAppearanceToFrame } from './app-appearance';
import { applyThemeMode, applyUiScale } from './preferences';

function appFrame(): { frame: HTMLIFrameElement; postMessage: ReturnType<typeof vi.fn> } {
  const frame = document.createElement('iframe');
  frame.setAttribute('data-role', 'app-ui-frame');
  document.body.appendChild(frame);
  const postMessage = vi.fn();
  Object.defineProperty(frame, 'contentWindow', { value: { postMessage } });
  return { frame, postMessage };
}

const pushesOf = (postMessage: ReturnType<typeof vi.fn>) =>
  postMessage.mock.calls.map(([message]) => message).filter(m => m.channel === APPEARANCE_CHANNEL);

describe('the appearance push to app frames', () => {
  beforeEach(() => {
    localStorage.clear();
  });

  afterEach(() => {
    document.body.innerHTML = '';
  });

  it('hands a loading frame the mirrors the shell painted', () => {
    localStorage.setItem('lucidos-ui-scale', '125');
    localStorage.setItem('lucidos-theme-mode', 'light');
    const { frame, postMessage } = appFrame();

    pushAppearanceToFrame(frame);

    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ channel: APPEARANCE_CHANNEL, data: { 'ui-scale': '125', 'theme-mode': 'light' } }),
      '*',
    );
  });

  it('pushes each paint to every open app frame, once per task', async () => {
    installAppAppearanceSync();
    await Promise.resolve();
    const first = appFrame();
    const second = appFrame();

    applyUiScale(150);
    applyThemeMode('light');
    await Promise.resolve();

    for (const { postMessage } of [first, second]) {
      const pushes = pushesOf(postMessage);
      expect(pushes).toHaveLength(1);
      expect(pushes[0].data).toMatchObject({ 'ui-scale': '150', 'theme-mode': 'light' });
    }
  });
});
