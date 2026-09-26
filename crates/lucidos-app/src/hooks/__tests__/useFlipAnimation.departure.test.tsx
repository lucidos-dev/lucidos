// @vitest-environment jsdom
/**
 * A row that leaves the drawer exits as a copy. A newer batch must not cut
 * that exit short: another row can land while the copy is still leaving, and
 * the row it stands for is gone either way. Every other animation of the older
 * batch is still taken over, as before.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { useFlipTransitions, type FlipSection } from '../useFlipAnimation';

const ROW_HEIGHT = 50;

interface FakeAnimation {
    target: Element;
    cancelled: boolean;
    finish: () => void;
}

let host: HTMLElement;
let animations: FakeAnimation[];

function Drawer({ ids }: { ids: string[] }) {
    const container = useRef<HTMLDivElement>(null);
    const portal = useRef<HTMLDivElement>(null);
    const sections: FlipSection[] = [{ name: 'current', ids }];
    useFlipTransitions(container, portal, sections);
    return (
        <div>
            <div ref={container} class="thread-drawer-rows">
                {ids.map(id => <div key={id} data-flip-id={id}>{id}</div>)}
            </div>
            <div ref={portal} />
        </div>
    );
}

/** A row sits at its index in the list. Anything else has no box. */
function rowRect(this: HTMLElement): DOMRect {
    const id = this.dataset.flipId;
    const rows = id ? [...this.parentElement!.querySelectorAll<HTMLElement>('[data-flip-id]')] : [];
    const top = id ? rows.indexOf(this) * ROW_HEIGHT : 0;
    const height = id ? ROW_HEIGHT : 0;
    return { top, left: 0, width: id ? 300 : 0, height, bottom: top + height, right: 300, x: 0, y: top, toJSON: () => ({}) } as DOMRect;
}

function fakeAnimate(this: Element): Animation {
    let resolve!: () => void;
    let reject!: (e: unknown) => void;
    const finished = new Promise<void>((res, rej) => { resolve = res; reject = rej; });
    finished.catch(() => {});
    const record: FakeAnimation = { target: this, cancelled: false, finish: () => resolve() };
    animations.push(record);
    return {
        finished,
        startTime: null,
        pause() {},
        play() {},
        cancel() {
            record.cancelled = true;
            reject(new DOMException('cancelled', 'AbortError'));
        },
    } as unknown as Animation;
}

async function nextFrame(): Promise<void> {
    await new Promise(resolve => setTimeout(resolve, 0));
}

describe('a departing row copy', () => {
    beforeEach(() => {
        animations = [];
        host = document.createElement('div');
        document.body.appendChild(host);
        vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(rowRect);
        Element.prototype.animate = fakeAnimate as typeof Element.prototype.animate;
        vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
    });

    afterEach(() => {
        render(null, host);
        host.remove();
        vi.restoreAllMocks();
        vi.unstubAllGlobals();
    });

    it('finishes its exit when a newer batch starts, then goes', async () => {
        render(<Drawer ids={['a', 'b', 'c']} />, host);
        render(<Drawer ids={['a', 'c']} />, host);

        const layer = host.querySelector('.flip-departure-layer');
        expect(layer, 'archiving b mounts a departure layer').not.toBeNull();
        const exit = animations.find(a => layer!.contains(a.target))!;
        const slide = animations.find(a => (a.target as HTMLElement).dataset?.flipId === 'c')!;
        expect(exit, 'the copy of b animates').toBeDefined();
        expect(slide, 'c slides up into the gap').toBeDefined();

        // Another row lands while b's copy is still leaving.
        render(<Drawer ids={['n', 'a', 'c']} />, host);
        await nextFrame();

        expect(slide.cancelled, 'the newer batch takes over the slide').toBe(true);
        expect(exit.cancelled, 'the exit runs on').toBe(false);
        expect(layer!.isConnected, 'the copy stays on screen').toBe(true);

        exit.finish();
        await nextFrame();
        expect(layer!.isConnected, 'the layer goes once its exit ends').toBe(false);
    });

    it('gives way to its own row when that row comes back mid-exit', async () => {
        render(<Drawer ids={['a', 'b', 'c']} />, host);
        render(<Drawer ids={['a', 'c']} />, host);
        const layer = host.querySelector('.flip-departure-layer')!;
        const exit = animations.find(a => layer.contains(a.target))!;

        // A rejected archive puts b straight back.
        render(<Drawer ids={['a', 'b', 'c']} />, host);
        await nextFrame();

        expect(exit.cancelled, 'the copy stops leaving').toBe(true);
        expect(layer.isConnected, 'and goes, so b shows once').toBe(false);
    });
});
