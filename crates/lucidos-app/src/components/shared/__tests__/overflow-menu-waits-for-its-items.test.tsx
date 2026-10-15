// @vitest-environment jsdom
/**
 * A menu whose rows are code-split opens only once they are in memory
 * (`OverflowMenu`'s `itemsChunk`, ADR 0436). The thread menu, the microphone
 * picker and the widget menus all ride it. A press or a host's gesture waits,
 * and a second press takes the waiting open back.
 */
import { describe, it, expect, afterEach } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { OverflowMenu, type OverflowMenuOpener } from '../OverflowMenu';

/** A chunk that lands when the test says. */
function coldChunk() {
  let finish: (loaded: boolean) => void = () => {};
  let loaded = false;
  const arrival = new Promise<boolean>((resolve) => { finish = resolve; });
  return {
    gate: { preload: () => arrival.then((ok) => { loaded = ok; return ok; }), isLoaded: () => loaded },
    land: async (ok = true) => {
      finish(ok);
      await act(async () => { await arrival; await new Promise((r) => setTimeout(r, 0)); });
    },
  };
}

const host = document.createElement('div');
document.body.appendChild(host);
afterEach(() => {
  act(() => { render(null, host); });
  document.querySelectorAll('.thread-overflow-menu').forEach((el) => el.remove());
});

const items = () => <button type="button" role="menuitem">Row</button>;
const menu = () => document.querySelector('.thread-overflow-menu [role="menuitem"]');
const trigger = () => host.querySelector<HTMLButtonElement>('button[aria-haspopup="menu"]')!;

describe('a menu whose rows are still on their way', () => {
  it('opens on a press only once its rows land', async () => {
    const chunk = coldChunk();
    act(() => { render(<OverflowMenu ariaLabel="Actions" itemsChunk={chunk.gate} items={items} />, host); });
    act(() => { trigger().click(); });
    expect(menu(), 'nothing opens before the rows arrive').toBeNull();
    await chunk.land();
    expect(menu()).not.toBeNull();
  });

  it('takes a waiting open back on a second press', async () => {
    const chunk = coldChunk();
    act(() => { render(<OverflowMenu ariaLabel="Actions" itemsChunk={chunk.gate} items={items} />, host); });
    act(() => { trigger().click(); });
    act(() => { trigger().click(); });
    await chunk.land();
    expect(menu()).toBeNull();
  });

  it('opens a host gesture only once its rows land', async () => {
    const chunk = coldChunk();
    const opener: { current: OverflowMenuOpener | null } = { current: null };
    const row = document.createElement('div');
    document.body.appendChild(row);
    act(() => {
      render(<OverflowMenu ariaLabel="Actions" hostOpener={{ ref: opener, trigger: false }} itemsChunk={chunk.gate} items={items} />, host);
    });
    act(() => { opener.current!(row); });
    expect(menu()).toBeNull();
    await chunk.land();
    expect(menu()).not.toBeNull();
    row.remove();
  });
});
