// @vitest-environment jsdom
/**
 * The thread ⋯ menu leads with Show in Folders. The item is present exactly
 * when the host passes `onShowInFolders`; which hosts do is pinned in
 * `drawer/__tests__/row-menu-show-in-folders.test.tsx`.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import { ThreadOverflowMenu } from '../ThreadOverflowMenu';

let host: HTMLElement | null = null;

function openMenu(onShowInFolders?: () => void): HTMLElement[] {
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<ThreadOverflowMenu threadId="t" title="T" onShowInFolders={onShowInFolders} />, host!); });
    const trigger = host.querySelector<HTMLElement>('button[aria-haspopup="menu"]');
    act(() => { trigger?.click(); });
    return [...document.querySelectorAll<HTMLElement>('.thread-overflow-menu [role="menuitem"]')];
}

afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    document.querySelectorAll('.thread-overflow-menu').forEach(el => el.remove());
});

describe('Show in Folders in the thread overflow menu', () => {
    it('leads the menu and runs the action when the host passes it', () => {
        const onShow = vi.fn();
        const items = openMenu(onShow);
        expect(items[0]?.textContent).toBe('Show in Folders');
        act(() => { items[0].click(); });
        expect(onShow).toHaveBeenCalledOnce();
    });

    it('is absent when the host does not pass it', () => {
        const labels = openMenu().map(el => el.textContent);
        expect(labels).not.toContain('Show in Folders');
        expect(labels.length).toBeGreaterThan(0);
    });
});
