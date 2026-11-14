// @vitest-environment jsdom
/**
 * The composer's rung control loads off the first frame (ADR 0436). Until its
 * code arrives the row draws the same button, and a press on it is held: the
 * control takes it as it mounts and opens, so the press is never lost.
 */
import { beforeAll, afterEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const chunk = vi.hoisted(() => {
  let release: () => void = () => {};
  const arrived = new Promise<void>((resolve) => { release = resolve; });
  return { arrived, release: () => release() };
});
vi.mock('../RungControl', async (importOriginal) => {
  await chunk.arrived;
  return importOriginal();
});
vi.mock('../../../store/actions/models', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/models')>()),
  loadChatModels: vi.fn(async () => {}),
}));

const { LazyRungControl } = await import('../LazyRungControl');
const { controlMenuPressed, pressControlMenu } = await import('../controlMenuRequests');
const { controlMenuParts } = await import('../controlMenuParts');

beforeAll(() => controlMenuParts.preload());

const host = document.createElement('div');
document.body.appendChild(host);
afterEach(() => { render(null, host); });

const settle = () => new Promise((r) => setTimeout(r, 0));

describe('the rung control before its code arrives', () => {
  it('draws its button, holds a press, and opens on it once the control lands', async () => {
    act(() => {
      render(<LazyRungControl codingAgent={null} composeContext />, host);
    });
    const button = host.querySelector<HTMLButtonElement>('.rung-trigger');
    expect(button, 'the placeholder draws the real button').not.toBeNull();
    expect(button!.getAttribute('aria-label')).toBe('Choose model and effort');
    expect(document.querySelector('.control-dropdown')).toBeNull();

    act(() => { button!.click(); });
    expect(controlMenuPressed.value).toBe('rung');

    chunk.release();
    await act(async () => {
      await LazyRungControl.preload();
      await settle();
    });
    expect(controlMenuPressed.value, 'the control took the press').toBeNull();
    expect(document.querySelector('.control-dropdown'), 'and opened on it').not.toBeNull();
  });
});

describe('a held press on a placeholder', () => {
  const button = document.createElement('button');
  document.body.appendChild(button);

  it('is taken back by a second press on the same button', () => {
    pressControlMenu('rung', button);
    pressControlMenu('rung', button);
    expect(controlMenuPressed.value).toBeNull();
  });

  it('is dropped by a press anywhere else', () => {
    pressControlMenu('rung', button);
    button.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    expect(controlMenuPressed.value, 'a press on its own button keeps it').toBe('rung');
    document.body.dispatchEvent(new PointerEvent('pointerdown', { bubbles: true }));
    expect(controlMenuPressed.value).toBeNull();
  });
});
