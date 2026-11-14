// @vitest-environment jsdom
import { describe, it, expect, beforeEach, afterEach } from 'vitest';
import { h, render, type RefObject } from 'preact';
import { useRef } from 'preact/hooks';
import { act } from 'preact/test-utils';
import { useScrollMemory } from './useScrollMemory';

/**
 * A pause takes effect from the render that asks for it. The effect that
 * detaches the memory runs only after paint. The swap that paused it clamps
 * the offset first: the Ongoing grouping replacing Folders, or a shut drawer
 * emptying its list. That clamp's scroll must not overwrite the saved place.
 */

const KEY = 'scroll-memory-paused-test';

let element: HTMLDivElement | null = null;

function Harness({ paused }: { paused: boolean }) {
    const ref = useRef<HTMLDivElement>(null);
    useScrollMemory(ref as RefObject<HTMLElement>, KEY, { paused });
    return h('div', {
        ref: (el: HTMLDivElement | null) => {
            ref.current = el;
            element = el;
        },
    });
}

/** Give the jsdom element the scroll box it lacks: a writable offset over a
 *  tall content. */
function scrollable(el: HTMLDivElement) {
    let top = 0;
    Object.defineProperty(el, 'scrollTop', { get: () => top, set: (v: number) => { top = v; }, configurable: true });
    Object.defineProperty(el, 'scrollHeight', { get: () => 5000, configurable: true });
    Object.defineProperty(el, 'clientHeight', { get: () => 800, configurable: true });
}

function scrollTo(el: HTMLDivElement, top: number) {
    el.scrollTop = top;
    el.dispatchEvent(new Event('scroll'));
}

/** Past the memory's save debounce. */
const pastSave = () => new Promise(resolve => setTimeout(resolve, 250));

describe('useScrollMemory pause', () => {
    let container: HTMLDivElement;

    beforeEach(() => {
        localStorage.clear();
        container = document.createElement('div');
        document.body.appendChild(container);
    });

    afterEach(() => {
        act(() => render(null, container));
        container.remove();
        element = null;
    });

    it('keeps the saved place when the paused render clamps the offset before the effect detaches', async () => {
        act(() => render(h(Harness, { paused: true }), container));
        scrollable(element!);
        act(() => render(h(Harness, { paused: false }), container));

        scrollTo(element!, 900);
        await pastSave();
        expect(localStorage.getItem(KEY)).toBe('900');

        // The paused render lands, then the swapped-in view clamps the offset
        // before the detaching effect has run.
        render(h(Harness, { paused: true }), container);
        scrollTo(element!, 0);
        await pastSave();

        expect(localStorage.getItem(KEY)).toBe('900');
    });
});
