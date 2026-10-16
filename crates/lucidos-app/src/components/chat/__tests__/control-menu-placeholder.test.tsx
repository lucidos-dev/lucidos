// @vitest-environment jsdom
/**
 * The composer's control menu loads off the first frame (ADR 0436). Until its
 * code arrives the row draws the same button, and a press on it is held: the
 * menu takes it as it mounts and opens, so the press is never lost.
 */
import { beforeAll, afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const chunk = vi.hoisted(() => {
  let release: () => void = () => {};
  const arrived = new Promise<void>((resolve) => { release = resolve; });
  return { arrived, release: () => release() };
});
vi.mock('../LucidosControlMenu', async (importOriginal) => {
  await chunk.arrived;
  return importOriginal();
});
vi.mock('../../../store/actions/models', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/models')>()),
  loadChatModels: vi.fn(async () => {}),
}));

const { LazyLucidosControlMenu, PromptRowControls } = await import('../PromptRowControls');
const { controlMenuPressed, pressControlMenu } = await import('../controlMenuRequests');
const { LUCIDOS_MENU_LABEL } = await import('../ControlMenuTrigger');
const { controlMenuParts } = await import('../controlMenuParts');

beforeAll(() => controlMenuParts.preload());

const host = document.createElement('div');
document.body.appendChild(host);
afterEach(() => { render(null, host); });

const settle = () => new Promise((r) => setTimeout(r, 0));

describe('the control menu before its code arrives', () => {
  it('draws its button, holds a press, and opens on it once the menu lands', async () => {
    act(() => {
      render(
        <PromptRowControls
          codingAgent={null} codingAgentThreadId={undefined} composeThreadId={undefined}
          lucidosThreadId={undefined} composeContext toggles={[]} attrsFor={() => ({})}
        />,
        host,
      );
    });
    const button = host.querySelector<HTMLButtonElement>('.commands-btn.lucidos-commands-btn');
    expect(button, 'the placeholder draws the real button').not.toBeNull();
    expect(button!.getAttribute('aria-label')).toBe(LUCIDOS_MENU_LABEL);
    expect(document.querySelector('.control-dropdown')).toBeNull();

    act(() => { button!.click(); });
    expect(controlMenuPressed.value).toBe('lucidos');

    chunk.release();
    await act(async () => {
      await LazyLucidosControlMenu.preload();
      await settle();
    });
    expect(controlMenuPressed.value, 'the menu took the press').toBeNull();
    expect(document.querySelector('.control-dropdown'), 'and opened on it').not.toBeNull();
  });
});

describe('a held press on a placeholder', () => {
  const button = document.createElement('button');
  document.body.appendChild(button);

  it('is taken back by a second press on the same button', () => {
    pressControlMenu('lucidos', button);
    pressControlMenu('lucidos', button);
    expect(controlMenuPressed.value).toBeNull();
  });

  it('is dropped by a press anywhere else', () => {
    pressControlMenu('lucidos', button);
    button.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    expect(controlMenuPressed.value, 'a press on its own button keeps it').toBe('lucidos');
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    expect(controlMenuPressed.value).toBeNull();
  });
});
