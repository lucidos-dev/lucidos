import { describe, it, expect, beforeEach, vi } from 'vitest';

const exitAppFullscreen = vi.fn(() => false);
vi.mock('./apps', () => ({ exitAppFullscreen }));

const openUrl = vi.fn();
vi.mock('./artifacts', () => ({ openUrl }));

const openOAuthAuthorizationUrl = vi.fn();
vi.mock('./oauth', () => ({ openOAuthAuthorizationUrl }));

const unfocusThread = vi.fn();
vi.mock('./threads', () => ({ unfocusThread }));

const ensureFocusedComposeThread = vi.fn(() => 'draft-1');
const updateCompose = vi.fn();
vi.mock('./compose', () => ({ ensureFocusedComposeThread, updateCompose }));

const prefillNewChatDestination = vi.fn();
vi.mock('./newChatPrefill', () => ({ prefillNewChatDestination }));

const focusPromptNow = vi.fn();
vi.mock('../../components/chat/promptFocus', () => ({ focusPromptNow }));

const isMobile = vi.fn(() => false);
vi.mock('../../utils/viewport', () => ({ isMobile }));

const showToast = vi.fn();
vi.mock('../store', async (importActual) => {
  const actual = await importActual<typeof import('../store')>();
  return { ...actual, showToast };
});

const { navigateToNewChat, navigateToUrl } = await import('./navigationDirect');

describe('navigateToUrl opens a link in the navigate\'s own tick', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('opens a link synchronously, with no await before the browser call', () => {
    navigateToUrl({ target: 'url', url: 'https://example.com' }, 'a thread');
    expect(openUrl).toHaveBeenCalledWith('https://example.com', 'a thread');
  });
});

describe('navigateToNewChat focuses the prompt after a frame', () => {
  beforeEach(() => {
    vi.clearAllMocks();
  });

  it('focuses the prompt only on the next frame, once Preact has committed the pane', () => {
    const frames: FrameRequestCallback[] = [];
    vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => {
      frames.push(cb);
      return frames.length;
    });
    try {
      navigateToNewChat({ target: 'new-chat' });
      expect(focusPromptNow).not.toHaveBeenCalled();
      frames.splice(0).forEach((cb) => cb(0));
      expect(focusPromptNow).toHaveBeenCalledTimes(1);
    } finally {
      vi.unstubAllGlobals();
    }
  });
});
