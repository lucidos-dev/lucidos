import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  isForwardableKeydown,
  toForwardedKeydown,
  installKeyboardForwarding,
  adoptHostBindings,
  matchesHostBinding,
  FORWARD_KEYDOWN_TYPE,
  KEYBINDINGS_CHANNEL,
} from './keyboardForward';

const SEARCH_FILES = { mod: true, shift: false, alt: false, key: 'p' };

describe('matchesHostBinding', () => {
  it('matches by the host rule, where Cmd and Ctrl both count', () => {
    expect(matchesHostBinding({ metaKey: true, ctrlKey: false, shiftKey: false, altKey: false, key: 'p' }, [SEARCH_FILES])).toBe(true);
    expect(matchesHostBinding({ metaKey: false, ctrlKey: true, shiftKey: false, altKey: false, key: 'P' }, [SEARCH_FILES])).toBe(true);
    expect(matchesHostBinding({ metaKey: true, ctrlKey: false, shiftKey: true, altKey: false, key: 'p' }, [SEARCH_FILES])).toBe(false);
  });
});

describe('isForwardableKeydown', () => {
  it('forwards primary-modifier chords (Cmd / Ctrl)', () => {
    expect(isForwardableKeydown({ metaKey: true, ctrlKey: false, altKey: false, key: '3' })).toBe(true);
    expect(isForwardableKeydown({ metaKey: false, ctrlKey: true, altKey: false, key: 's' })).toBe(true);
  });

  it('forwards Alt chords (the arrow pane-resize shortcuts)', () => {
    expect(isForwardableKeydown({ metaKey: true, ctrlKey: false, altKey: true, key: 'ArrowLeft' })).toBe(true);
  });

  it('forwards Escape with no modifier', () => {
    expect(isForwardableKeydown({ metaKey: false, ctrlKey: false, altKey: false, key: 'Escape' })).toBe(true);
  });

  it('does not forward plain typing', () => {
    expect(isForwardableKeydown({ metaKey: false, ctrlKey: false, altKey: false, key: 'a' })).toBe(false);
    expect(isForwardableKeydown({ metaKey: false, ctrlKey: false, altKey: false, key: 'Enter' })).toBe(false);
  });

  it('forwards an F-key, so F2 renames a thread while an app has focus', () => {
    expect(isForwardableKeydown({ metaKey: false, ctrlKey: false, altKey: false, key: 'F2' })).toBe(true);
    expect(isForwardableKeydown({ metaKey: false, ctrlKey: false, altKey: false, key: 'F12' })).toBe(true);
  });

  it('leaves Ctrl+letter to a Mac text field, where it edits text', () => {
    const inField = { metaKey: false, ctrlKey: true, altKey: false, shiftKey: false, key: 'k', target: { tagName: 'TEXTAREA' } as unknown as EventTarget };
    expect(isForwardableKeydown(inField, true)).toBe(false);
    expect(isForwardableKeydown({ ...inField, metaKey: true, ctrlKey: false }, true)).toBe(true);
    expect(isForwardableKeydown(inField, false)).toBe(true);
    expect(isForwardableKeydown({ ...inField, target: { tagName: 'DIV' } as unknown as EventTarget }, true)).toBe(true);
  });

  it('does not forward Shift-only chords', () => {
    // A Shift+letter is just an uppercase keystroke to the app — `shiftKey`
    // isn't even read, so it can never tip a bare letter into being forwarded.
    expect(isForwardableKeydown({ metaKey: false, ctrlKey: false, altKey: false, key: 'A' })).toBe(false);
  });
});

describe('toForwardedKeydown', () => {
  it('captures the chord shape the host matcher needs', () => {
    const e = { key: '3', metaKey: true, ctrlKey: false, shiftKey: true, altKey: false } as KeyboardEvent;
    expect(toForwardedKeydown(e)).toEqual({
      type: FORWARD_KEYDOWN_TYPE,
      key: '3',
      metaKey: true,
      ctrlKey: false,
      shiftKey: true,
      altKey: false,
    });
  });
});

describe('installKeyboardForwarding', () => {
  let cleanup: () => void = () => {};

  // The test env has no `KeyboardEvent`; dispatch a real `Event` carrying the
  // chord props the forwarder reads (the codebase's no-jsdom convention).
  function fireKeydown(props: Partial<KeyboardEvent>) {
    const e = new Event('keydown');
    Object.assign(e, props);
    window.dispatchEvent(e);
  }

  function withParent(): ReturnType<typeof vi.fn> {
    const postMessage = vi.fn();
    (window as unknown as { parent: unknown }).parent = { postMessage };
    return postMessage;
  }

  afterEach(() => {
    cleanup();
    cleanup = () => {};
    delete (window as unknown as { parent?: unknown }).parent;
  });

  it('forwards a modifier chord to the parent', () => {
    const postMessage = withParent();
    cleanup = installKeyboardForwarding();
    fireKeydown({ key: '3', metaKey: true, shiftKey: true });
    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ type: FORWARD_KEYDOWN_TYPE, key: '3', metaKey: true, shiftKey: true }),
      '*',
    );
  });

  it('forwards Escape to the parent', () => {
    const postMessage = withParent();
    cleanup = installKeyboardForwarding();
    fireKeydown({ key: 'Escape' });
    expect(postMessage).toHaveBeenCalledTimes(1);
    expect(postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ type: FORWARD_KEYDOWN_TYPE, key: 'Escape' }),
      '*',
    );
  });

  it('does not forward plain typing', () => {
    const postMessage = withParent();
    cleanup = installKeyboardForwarding();
    fireKeydown({ key: 'a' });
    expect(postMessage).not.toHaveBeenCalled();
  });

  it('no-ops when there is no parent window (SDK loaded top-level)', () => {
    // Self-parenting is the top-level signal — nothing to forward to.
    (window as unknown as { parent: unknown }).parent = window;
    const post = vi.fn();
    (window as unknown as { postMessage: unknown }).postMessage = post;
    cleanup = installKeyboardForwarding();
    fireKeydown({ key: '3', metaKey: true });
    expect(post).not.toHaveBeenCalled();
  });

  it('cancels the browser default for a host-bound chord, so ⌘P is not a print', () => {
    const parent = { postMessage: vi.fn() };
    (window as unknown as { parent: unknown }).parent = parent;
    cleanup = installKeyboardForwarding();
    // The host's push, as _bridge.ts receives it: from the parent, on the channel.
    const push = new Event('message');
    Object.assign(push, { source: parent, data: { type: 'lucidos:bridge:push', channel: KEYBINDINGS_CHANNEL, data: { bindings: [SEARCH_FILES] } } });
    window.dispatchEvent(push);
    const bound = new Event('keydown', { cancelable: true });
    Object.assign(bound, { key: 'p', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false });
    window.dispatchEvent(bound);
    const unbound = new Event('keydown', { cancelable: true });
    Object.assign(unbound, { key: 'c', metaKey: true, ctrlKey: false, shiftKey: false, altKey: false });
    window.dispatchEvent(unbound);
    expect(bound.defaultPrevented).toBe(true);
    expect(unbound.defaultPrevented).toBe(false);
    adoptHostBindings({ bindings: [] });
  });

  it('drops a malformed push rather than throwing', () => {
    adoptHostBindings({ bindings: [{ mod: 'yes', key: 'p' }, null, SEARCH_FILES] });
    expect(matchesHostBinding({ metaKey: true, ctrlKey: false, shiftKey: false, altKey: false, key: 'p' })).toBe(true);
    expect(matchesHostBinding({ metaKey: false, ctrlKey: false, shiftKey: false, altKey: false, key: 'p' })).toBe(false);
    adoptHostBindings(null);
    expect(matchesHostBinding({ metaKey: true, ctrlKey: false, shiftKey: false, altKey: false, key: 'p' })).toBe(false);
  });

  it('cleanup removes the listener', () => {
    const postMessage = withParent();
    const stop = installKeyboardForwarding();
    stop();
    fireKeydown({ key: '3', metaKey: true });
    expect(postMessage).not.toHaveBeenCalled();
  });
});
