// @vitest-environment jsdom
/** A folded coding-agent menu draws no button, yet every way in still opens it.
 *
 *  The composer row folds the menu into the ⋯ first of all. Three paths must
 *  still open the panel: the ⋯ row, a `/` typed in the prompt, and the global
 *  `/` shortcut. Each anchors it to the ⋯ trigger, which stands in for the
 *  button. */
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const { CodingAgentControlMenu } = await import('../CodingAgentControlMenu');
const {
  closeControlMenus, codingAgentCommandsReady, codingAgentFoldOpenRequest, codingAgentMenuOpenRequest,
} = await import('../controlMenuRequests');
const { codingAgentMenuAction } = await import('../PromptRowControls');
const { toasts } = await import('../../../store/store');
const { controlMenuParts } = await import('../controlMenuParts');

beforeAll(() => controlMenuParts.preload());

let row: HTMLDivElement;
let more: HTMLButtonElement;
let host: HTMLDivElement;

let builtins = ['compact'];

beforeEach(() => {
  builtins = ['compact'];
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({
    control_commands: [],
    builtin_commands: builtins,
    skill_commands: [],
    current_model: null,
    current_reasoning_effort: null,
    has_active_session: true,
    ladder_rungs: [],
  }), { status: 200 })));
  row = document.createElement('div');
  row.className = 'prompt-actions-row';
  more = document.createElement('button');
  more.setAttribute('data-fold-key', '');
  // jsdom lays nothing out. The menu waits for a ⋯ with a real size, inside
  // ancestors with one too.
  vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockReturnValue({
    width: 30, height: 30, top: 0, left: 0, right: 30, bottom: 30, x: 0, y: 0, toJSON: () => ({}),
  });
  host = document.createElement('div');
  row.append(more, host);
  document.body.appendChild(row);
});

afterEach(() => {
  render(null, host);
  row.remove();
  codingAgentFoldOpenRequest.value = null;
  codingAgentMenuOpenRequest.value = null;
  toasts.value = [];
  vi.unstubAllGlobals();
  vi.restoreAllMocks();
});

const frame = () => act(() => new Promise((r) => requestAnimationFrame(() => r(undefined))));

/** A compose mount drops the cached command list, which an earlier test's
 *  thread mount left behind. */
async function mountFolded({ compose = false } = {}) {
  const threadId = compose ? undefined : 'thread-1';
  await act(async () => {
    render(
      <CodingAgentControlMenu
        threadId={threadId}
        codingAgent="claude-code"
        folded
        rowAttrs={{ 'data-row-item': 'fold', 'data-fold-key': 'coding-agent-menu' }}
      />,
      host,
    );
  });
  await act(async () => { await new Promise((r) => setTimeout(r, 0)); });
  await frame();
}

const panelOpen = () => document.querySelector('.control-item') !== null;

describe('a folded coding-agent menu', () => {
  it('draws no button of its own', async () => {
    await mountFolded();
    expect(host.querySelector('.commands-btn')).toBeNull();
    expect(panelOpen()).toBe(false);
  });

  it('opens from its ⋯ row', async () => {
    await mountFolded();
    act(() => { codingAgentFoldOpenRequest.value = more; });
    await frame();
    expect(panelOpen()).toBe(true);
  });

  /** The panel hangs from the ⋯ and ignores a press on it, so the ⋯ opening
   *  must shut it, or the two stack. */
  it('closes when the ⋯ asks the control menus to close', async () => {
    await mountFolded();
    act(() => { codingAgentFoldOpenRequest.value = more; });
    await frame();
    act(() => closeControlMenus());
    await frame();
    expect(panelOpen()).toBe(false);
  });

  it('opens from a "/" typed in the prompt', async () => {
    await mountFolded();
    act(() => { codingAgentMenuOpenRequest.value = ''; });
    await frame();
    expect(panelOpen()).toBe(true);
    expect(codingAgentMenuOpenRequest.value, 'the menu took the request').toBeNull();
  });

  it('opens from the global "/" shortcut', async () => {
    await mountFolded();
    act(() => {
      document.body.dispatchEvent(new KeyboardEvent('keydown', { key: '/', bubbles: true }));
    });
    await frame();
    expect(panelOpen()).toBe(true);
  });

  it('dims its ⋯ row until the commands arrive, as the button does', async () => {
    expect(codingAgentMenuAction('claude-code')?.extraClass).toBe('commands-btn');
    await mountFolded();
    expect(codingAgentCommandsReady.value).toBe(true);
    expect(codingAgentMenuAction('claude-code')?.extraClass).toBe('commands-btn commands-btn-active');
  });

  it('says why its ⋯ row opened nothing before the commands arrive', async () => {
    builtins = [];
    await mountFolded({ compose: true });
    act(() => { codingAgentFoldOpenRequest.value = more; });
    await frame();
    expect(panelOpen()).toBe(false);
    expect(toasts.value.map((t) => t.message)).toContain('Claude Code commands are still loading');
  });
});
