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
import { ThreadTitleMenu } from '../../chat/ThreadTitle';
import { FindBar } from '../FindBar';
import { resetFind } from '../../../store/actions/find-bar';
import type { ThreadState } from '../../../store/thread-events';

let host: HTMLElement | null = null;
const realMatchMedia = window.matchMedia;

function mount(vnode: preact.ComponentChild): HTMLElement {
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(vnode, host!); });
    return host;
}

function openMenu(onFindInThread?: () => void): HTMLElement[] {
    const root = mount(<ThreadOverflowMenu threadId="t" title="T" onFindInThread={onFindInThread} />);
    const trigger = root.querySelector<HTMLElement>('button[aria-haspopup="menu"]');
    act(() => { trigger?.click(); });
    return menuItems();
}

function menuItems(): HTMLElement[] {
    return [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')];
}

/** A finger: the only pointer with an on-screen keyboard. */
function useCoarsePointer(): void {
    window.matchMedia = vi.fn((query: string) => ({
        matches: query === '(pointer: coarse)',
        addEventListener: () => {},
        removeEventListener: () => {},
    })) as unknown as typeof window.matchMedia;
}

afterEach(() => {
    if (host) { act(() => { render(null, host!); }); host.remove(); host = null; }
    document.querySelectorAll('.thread-overflow-menu, [data-keyboard-proxy]').forEach(el => el.remove());
    window.matchMedia = realMatchMedia;
    resetFind();
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

// iOS raises the keyboard only for a focus made inside the tap itself. The
// find field mounts a render later, so a text field must already hold focus
// when the item's click returns: no render, effect, frame or timer between.
describe('Find in thread on a phone', () => {
    const thread = { meta: { id: 't', state: 'active', home: false } } as unknown as ThreadState;

    function tapFindInThread(): Element | null {
        useCoarsePointer();
        const root = mount(<>
            <ThreadTitleMenu thread={thread} title="T" status={null} />
            <FindBar surface="thread" scope="thread:t" placeholder="Find in thread" />
        </>);
        act(() => { root.querySelector<HTMLElement>('.thread-title-menu')!.click(); });
        const item = menuItems().find((el) => el.textContent === 'Find in thread')!;
        let focusedInTap: Element | null = null;
        // A window listener runs last in the click's own dispatch.
        window.addEventListener('click', () => { focusedInTap = document.activeElement; }, { once: true });
        item.click();
        return focusedInTap;
    }

    it('focuses a text field inside the tap, before the bar renders', () => {
        const focused = tapFindInThread();
        expect(focused?.tagName).toBe('INPUT');
        expect(host!.querySelector('[data-role="find-input"]')).toBeNull();
    });

    it('hands that focus to the find field once the bar renders', async () => {
        tapFindInThread();
        await act(async () => {});
        expect(document.activeElement).toBe(host!.querySelector('[data-role="find-input"]'));
    });
});
