import { describe, it, expect, vi, afterEach } from 'vitest';
import { installScaleWheelForwarding, FORWARD_WHEEL_TYPE, SCALE_WHEEL_CHANNEL } from './scaleWheelForward';

const HOST_ORIGIN = 'https://localhost:5251';

function fire(type: string, props: Record<string, unknown>): Event {
  const e = new Event(type, { cancelable: true });
  Object.assign(e, props);
  window.dispatchEvent(e);
  return e;
}

function wheel(props: Record<string, unknown>): Event {
  return fire('wheel', { deltaY: -100, deltaMode: 0, metaKey: false, ctrlKey: false, ...props });
}

describe('installScaleWheelForwarding', () => {
  let cleanup: () => void = () => {};
  let parent: { postMessage: ReturnType<typeof vi.fn> };

  function install() {
    parent = { postMessage: vi.fn() };
    (window as unknown as { parent: unknown }).parent = parent;
    vi.stubGlobal('location', { origin: HOST_ORIGIN });
    cleanup = installScaleWheelForwarding();
  }

  function hostSaysArmed(armed: boolean) {
    fire('message', {
      source: parent,
      origin: HOST_ORIGIN,
      data: { type: 'lucidos:bridge:push', channel: SCALE_WHEEL_CHANNEL, data: { armed } },
    });
  }

  afterEach(() => {
    cleanup();
    cleanup = () => {};
    delete (window as unknown as { parent?: unknown }).parent;
    vi.unstubAllGlobals();
  });

  it('forwards Cmd + wheel to the host once the frame saw Cmd go down', () => {
    install();
    fire('keydown', { key: 'Meta', metaKey: true });
    const e = wheel({ metaKey: true });
    expect(e.defaultPrevented).toBe(true);
    expect(parent.postMessage).toHaveBeenCalledWith(
      { type: FORWARD_WHEEL_TYPE, deltaY: -100, deltaMode: 0, metaKey: true, ctrlKey: false, modifierKeyHeld: true },
      HOST_ORIGIN,
    );
  });

  it('is not listening with no modifier seen and the host not armed', () => {
    install();
    const e = wheel({ ctrlKey: true });
    expect(e.defaultPrevented).toBe(false);
    expect(parent.postMessage).not.toHaveBeenCalled();
  });

  it('forwards a pinch while the host says it is listening', () => {
    install();
    hostSaysArmed(true);
    wheel({ deltaY: -4, ctrlKey: true });
    expect(parent.postMessage).toHaveBeenCalledWith(
      expect.objectContaining({ type: FORWARD_WHEEL_TYPE, ctrlKey: true, modifierKeyHeld: false }),
      HOST_ORIGIN,
    );
    hostSaysArmed(false);
    parent.postMessage.mockClear();
    wheel({ deltaY: -4, ctrlKey: true });
    expect(parent.postMessage).not.toHaveBeenCalled();
  });

  it('leaves a plain scroll alone even while armed', () => {
    install();
    hostSaysArmed(true);
    expect(wheel({}).defaultPrevented).toBe(false);
    expect(parent.postMessage).not.toHaveBeenCalled();
  });

  it('leaves a wheel the app claimed for itself', () => {
    const claim = (e: Event) => e.preventDefault();
    window.addEventListener('wheel', claim);
    install();
    fire('keydown', { key: 'Meta', metaKey: true });
    wheel({ metaKey: true });
    window.removeEventListener('wheel', claim);
    expect(parent.postMessage).not.toHaveBeenCalled();
  });

  it('stops listening on keyup and on blur', () => {
    install();
    fire('keydown', { key: 'Meta', metaKey: true });
    fire('keyup', { key: 'Meta', metaKey: false, ctrlKey: false });
    wheel({ metaKey: true });
    fire('keydown', { key: 'Control', ctrlKey: true });
    fire('blur', {});
    wheel({ ctrlKey: true });
    expect(parent.postMessage).not.toHaveBeenCalled();
  });

  it('no-ops at the top level, with no host to tell', () => {
    (window as unknown as { parent: unknown }).parent = window;
    cleanup = installScaleWheelForwarding();
    fire('keydown', { key: 'Meta', metaKey: true });
    expect(wheel({ metaKey: true }).defaultPrevented).toBe(false);
  });

  it('cleanup removes every listener', () => {
    install();
    fire('keydown', { key: 'Meta', metaKey: true });
    cleanup();
    expect(wheel({ metaKey: true }).defaultPrevented).toBe(false);
    expect(parent.postMessage).not.toHaveBeenCalled();
  });
});
