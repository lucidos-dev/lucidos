import { describe, it, expect, vi, afterEach } from 'vitest';
import {
  isForwardableKeydown,
  toForwardedKeydown,
  installKeyboardForwarding,
  FORWARD_KEYDOWN_TYPE,
} from './keyboardForward';

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

  it('does not forward Shift-only chords (no host shortcut is Shift-only)', () => {
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

  it('cleanup removes the listener', () => {
    const postMessage = withParent();
    const stop = installKeyboardForwarding();
    stop();
    fireKeydown({ key: '3', metaKey: true });
    expect(postMessage).not.toHaveBeenCalled();
  });
});
