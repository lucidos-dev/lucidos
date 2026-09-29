import { describe, expect, it, vi } from 'vitest';
import { APP_KEYBINDINGS_CHANNEL, pushKeybindingsToFrame } from './app-keybindings';
import { allBindings } from './keybindings';
import { KEYBINDINGS_CHANNEL } from '../../../../../packages/lucidos-sdk/src/keyboardForward';

describe('pushKeybindingsToFrame', () => {
  it('speaks the channel the SDK listens on', () => {
    expect(APP_KEYBINDINGS_CHANNEL).toBe(KEYBINDINGS_CHANNEL);
  });

  it('hands the frame every current binding on the keybindings channel', () => {
    const postMessage = vi.fn();
    const frame = { contentWindow: { postMessage } } as unknown as HTMLIFrameElement;
    pushKeybindingsToFrame(frame);
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ channel: APP_KEYBINDINGS_CHANNEL, data: { bindings: allBindings() } }),
      '*',
    );
  });
});
