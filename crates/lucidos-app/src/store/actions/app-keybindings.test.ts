import { describe, expect, it, vi } from 'vitest';
import { APP_KEYBINDINGS_CHANNEL, pushKeybindingsToFrame } from './app-keybindings';
import { forwardableBindings } from './keybindings';
import { serializeBinding, shortcutDef } from '../../utils/shortcuts';
import { KEYBINDINGS_CHANNEL } from '../../../../../packages/lucidos-sdk/src/keyboardForward';

describe('pushKeybindingsToFrame', () => {
  it('speaks the channel the SDK listens on', () => {
    expect(APP_KEYBINDINGS_CHANNEL).toBe(KEYBINDINGS_CHANNEL);
  });

  it('never hands a frame the host-only Apply chord', () => {
    const apply = serializeBinding(shortcutDef('applyChange').defaultBinding);
    expect(forwardableBindings().map(serializeBinding)).not.toContain(apply);
    expect(forwardableBindings().map(serializeBinding)).toContain(serializeBinding(shortcutDef('followLiveEdge').defaultBinding));
  });

  it('hands the frame every forwardable binding on the keybindings channel', () => {
    const postMessage = vi.fn();
    const frame = { contentWindow: { postMessage } } as unknown as HTMLIFrameElement;
    pushKeybindingsToFrame(frame);
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ channel: APP_KEYBINDINGS_CHANNEL, data: { bindings: forwardableBindings() } }),
      '*',
    );
  });
});
