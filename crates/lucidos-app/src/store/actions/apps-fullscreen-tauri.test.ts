import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { appPseudoFullscreen } from '../store';

// The desktop client fullscreens the WINDOW, whatever way the toggle arrives.
// The frontend test environment cannot drive WKWebView (ADR 0016), so these pin
// the page's half: which commands go out, and when.

vi.mock('../../api/client', () => ({}));
vi.mock('./navigation', () => ({ pushNavState: vi.fn(), replaceNavState: vi.fn() }));
vi.mock('../../utils/elementVisibility', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/elementVisibility')>()),
  isElementVisible: () => true,
}));
vi.mock('../../utils/platform', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/platform')>()),
  isTauri: vi.fn(() => true),
}));
const setWindowFullscreen = vi.fn<(fullscreen: boolean) => Promise<boolean>>();
let fullscreenChanged: ((e: { payload: boolean }) => void) | null = null;
vi.mock('../../utils/tauri', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../utils/tauri')>()),
  setWindowFullscreen: (fullscreen: boolean) => setWindowFullscreen(fullscreen),
  listen: vi.fn(async (_event: string, handler: (e: { payload: boolean }) => void) => {
    fullscreenChanged = handler;
    return () => {};
  }),
}));

const { isTauri } = await import('../../utils/platform');
const { toggleAppFullscreen, installAppFullscreenWindowSync } = await import('./apps');

const requestFullscreen = vi.fn();
const panel = { requestFullscreen };
const frame = { contentWindow: {}, focus: vi.fn(), closest: () => panel } as unknown as HTMLIFrameElement;
const settle = () => new Promise((resolve) => setTimeout(resolve, 0));

let origQuerySelectorAll: typeof document.querySelectorAll;

beforeEach(async () => {
  appPseudoFullscreen.value = false;
  await settle();
  // The shell's report of the exit the line above caused.
  fullscreenChanged?.({ payload: false });
  origQuerySelectorAll = document.querySelectorAll;
  document.querySelectorAll = (() => [frame]) as unknown as typeof document.querySelectorAll;
  vi.mocked(isTauri).mockReturnValue(true);
  setWindowFullscreen.mockReset().mockResolvedValue(false);
  requestFullscreen.mockReset();
});

afterEach(() => {
  document.querySelectorAll = origQuerySelectorAll;
});

// One subscriber for the whole file: the signal keeps it, as it does in the app.
installAppFullscreenWindowSync();

describe('toggleAppFullscreen in the desktop client', () => {
  it('enters pseudo-fullscreen and a fullscreen window, never element fullscreen', async () => {
    toggleAppFullscreen();
    await settle();
    expect(appPseudoFullscreen.value).toBe(true);
    expect(setWindowFullscreen).toHaveBeenCalledWith(true);
    expect(requestFullscreen).not.toHaveBeenCalled();
  });

  it('takes the window back out when it entered it', async () => {
    toggleAppFullscreen();
    toggleAppFullscreen();
    await settle();
    expect(appPseudoFullscreen.value).toBe(false);
    expect(setWindowFullscreen.mock.calls).toEqual([[true], [false]]);
  });

  it('leaves a window the user already made fullscreen as it found it', async () => {
    setWindowFullscreen.mockResolvedValue(true);
    toggleAppFullscreen();
    toggleAppFullscreen();
    await settle();
    expect(setWindowFullscreen.mock.calls).toEqual([[true]]);
  });

  it('takes the window out when pseudo-fullscreen ends some other way, like Escape', async () => {
    toggleAppFullscreen();
    await settle();
    appPseudoFullscreen.value = false;
    await settle();
    expect(setWindowFullscreen.mock.calls).toEqual([[true], [false]]);
  });

  it('drops pseudo-fullscreen when macOS leaves window fullscreen', async () => {
    toggleAppFullscreen();
    await settle();
    fullscreenChanged?.({ payload: true });
    expect(appPseudoFullscreen.value).toBe(true);
    fullscreenChanged?.({ payload: false });
    expect(appPseudoFullscreen.value).toBe(false);
  });
});

describe('a quick toggle pair', () => {
  // The exit's report arrives after the re-entry, and the re-entry saw a window
  // still fullscreen. The page's own exit must not read as the user leaving.
  it('keeps pseudo-fullscreen and the window it owns when the exit report arrives late', async () => {
    setWindowFullscreen.mockResolvedValueOnce(false).mockResolvedValueOnce(false).mockResolvedValueOnce(true);
    toggleAppFullscreen();
    toggleAppFullscreen();
    toggleAppFullscreen();
    await settle();
    fullscreenChanged?.({ payload: false });
    expect(appPseudoFullscreen.value).toBe(true);
    toggleAppFullscreen();
    await settle();
    expect(setWindowFullscreen.mock.calls).toEqual([[true], [false], [true], [false]]);
  });

  it('sends the entry before the exit even when the entry answers slowly', async () => {
    let answer: (wasFullscreen: boolean) => void = () => {};
    setWindowFullscreen.mockReturnValueOnce(new Promise<boolean>((resolve) => { answer = resolve; }));
    toggleAppFullscreen();
    toggleAppFullscreen();
    await settle();
    expect(setWindowFullscreen.mock.calls).toEqual([[true]]);
    answer(false);
    await settle();
    expect(setWindowFullscreen.mock.calls).toEqual([[true], [false]]);
  });
});

describe('leaving window fullscreen by macOS', () => {
  it('sends no exit command, and leaves no echo to swallow the next macOS exit', async () => {
    toggleAppFullscreen();
    await settle();
    fullscreenChanged?.({ payload: false });
    await settle();
    expect(setWindowFullscreen.mock.calls).toEqual([[true]]);
    toggleAppFullscreen();
    await settle();
    fullscreenChanged?.({ payload: false });
    expect(appPseudoFullscreen.value).toBe(false);
  });
});

describe('toggleAppFullscreen in a browser', () => {
  it('asks the panel for element fullscreen and leaves the window alone', async () => {
    vi.mocked(isTauri).mockReturnValue(false);
    requestFullscreen.mockResolvedValue(undefined);
    toggleAppFullscreen();
    await settle();
    expect(requestFullscreen).toHaveBeenCalled();
    expect(setWindowFullscreen).not.toHaveBeenCalled();
  });
});
