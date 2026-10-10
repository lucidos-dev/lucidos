// @vitest-environment jsdom
/**
 * The thread ⋯ menu's Apply on settle row is a toggle. Armed, it draws its
 * check at the row's END, after the words, so the label keeps the column its
 * siblings' icons give it. The label itself carries no check glyph.
 */
import { describe, it, expect, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { act } from 'preact/test-utils';
import type { TaggedAction } from '../../../store/actions/threadActions';

const THREAD = 't';

vi.mock('../../../store/actions/threadActions', async (importOriginal) => {
    const actual = await importOriginal<typeof import('../../../store/actions/threadActions')>();
    const { standingApplyThreadIds } = await import('../../../store/store');
    return {
        ...actual,
        resolveChangeMenuActions: (): TaggedAction[] => {
            const armed = standingApplyThreadIds.value.has(THREAD);
            return [{
                kind: 'apply_when_settled',
                category: 'primary',
                label: armed ? 'Applying on settle' : 'Apply on settle',
                invoke: () => {},
            }];
        },
    };
});

import { ThreadOverflowMenu } from '../ThreadOverflowMenu';
import { standingApplyThreadIds } from '../../../store/store';

let host: HTMLElement | null = null;

function standingApplyRow(): HTMLElement {
    host = document.createElement('div');
    document.body.appendChild(host);
    act(() => { render(<ThreadOverflowMenu threadId={THREAD} title="T" />, host!); });
    act(() => { host!.querySelector<HTMLElement>('button[aria-haspopup="menu"]')?.click(); });
    const row = document.querySelector<HTMLElement>('.thread-overflow-menu [role="menuitemcheckbox"]');
    if (!row) throw new Error('no Apply on settle row');
    return row;
}

afterEach(() => {
    if (host) { render(null, host); host.remove(); host = null; }
    document.querySelectorAll('.thread-overflow-menu').forEach(el => el.remove());
    standingApplyThreadIds.value = new Set();
});

describe('Apply on settle in the thread overflow menu', () => {
    it('draws the armed check after the label, at the row end', () => {
        standingApplyThreadIds.value = new Set([THREAD]);
        const row = standingApplyRow();
        expect(row.getAttribute('aria-checked')).toBe('true');
        const check = row.lastElementChild;
        expect(check?.classList.contains('thread-overflow-check-end')).toBe(true);
        expect(check?.textContent).toBe('✓');
        expect(row.textContent).toBe('Applying on settle✓');
    });

    it('draws no check while unarmed', () => {
        const row = standingApplyRow();
        expect(row.getAttribute('aria-checked')).toBe('false');
        expect(row.querySelector('.thread-overflow-check')).toBeNull();
        expect(row.textContent).toBe('Apply on settle');
    });
});
