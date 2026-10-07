// @vitest-environment jsdom
/**
 * The thread title's ⋯ menu carries Find in thread, the route to the
 * transcript's find bar on a phone. A drawer row's menu does not: the item is
 * present exactly when the host passes `onFindInThread`.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadOverflowMenu } from '../ThreadOverflowMenu';

let host: HTMLElement | null = null;

function openMenu(onFindInThread?: () => void): HTMLElement[] {
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<ThreadOverflowMenu threadId="t" title="T" onFindInThread={onFindInThread} />, host!); });
    const trigger = host.querySelector<HTMLElement>('button[aria-haspopup="menu"]');
    act(() => { trigger?.click(); });
    return [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')];
}

afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    document.querySelectorAll('.thread-overflow-menu').forEach(el => el.remove());
});

describe('Find in thread in the thread overflow menu', () => {
    it('opens the find bar when the host passes it', () => {
        const onFind = vi.fn();
        const item = openMenu(onFind).find((el) => el.textContent === 'Find in thread');
        expect(item).toBeDefined();
        act(() => { item!.click(); });
        expect(onFind).toHaveBeenCalledOnce();
    });

    it('is absent when the host does not pass it', () => {
        expect(openMenu().map((el) => el.textContent)).not.toContain('Find in thread');
    });
});
