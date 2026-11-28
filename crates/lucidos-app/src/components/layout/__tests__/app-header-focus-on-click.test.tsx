// @vitest-environment jsdom
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';

const focusPane = vi.hoisted(() => vi.fn());
vi.mock('../../../store/actions/pane', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../../store/actions/pane')>()),
  focusPane,
}));

const { AppHeader } = await import('../AppHeader');

let host: HTMLDivElement | undefined;

beforeEach(() => {
  focusPane.mockClear();
  host = document.createElement('div');
  document.body.append(host);
});

afterEach(() => {
  if (host) { render(null, host); host.remove(); host = undefined; }
});

async function mountHeader(): Promise<HTMLElement> {
  await act(async () => { render(<AppHeader />, host!); });
  return host!.querySelector('header') as HTMLElement;
}

function pointerdown(el: Element): void {
  el.dispatchEvent(new Event('pointerdown', { bubbles: true }));
}

describe('header regions set the focused pane on click, not on press', () => {
  it('the content header does not move focus on a press alone', async () => {
    await mountHeader();
    pointerdown(host!.querySelector('.content-header-elements')!);
    expect(focusPane).not.toHaveBeenCalled();
  });

  it('the content header moves focus to the content pane on a click', async () => {
    await mountHeader();
    host!.querySelector('.content-header-elements')!.dispatchEvent(new Event('click', { bubbles: true }));
    expect(focusPane).toHaveBeenCalledWith('content');
  });

  it('the threads header does not move focus on a press alone', async () => {
    await mountHeader();
    pointerdown(host!.querySelector('.threads-header')!);
    expect(focusPane).not.toHaveBeenCalled();
  });

  it('the threads header moves focus to the drawer on a click', async () => {
    await mountHeader();
    host!.querySelector('.threads-header')!.dispatchEvent(new Event('click', { bubbles: true }));
    expect(focusPane).toHaveBeenCalledWith('drawer');
  });

  it('the thread brand region does not move focus on a press alone', async () => {
    await mountHeader();
    pointerdown(host!.querySelector('.pane-header-brand')!);
    expect(focusPane).not.toHaveBeenCalled();
  });
});
