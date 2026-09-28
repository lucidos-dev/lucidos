// @vitest-environment jsdom
/**
 * Rows a disclosure rolls under their parent dissolve into it rather than
 * meeting a hard edge. The mask's fade reaches only as far as the top copy has
 * slid under the line, so a row at rest is never dimmed.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { useFlipTransitions, type FlipSection } from '../useFlipAnimation';
import { FADE_REACH } from '../../utils/disclosureMotion';

const ROW_HEIGHT = 50;

let host: HTMLElement;
let animations: { target: Element; keyframes: Keyframe[] }[];

function Drawer({ ids, open }: { ids: string[]; open: boolean }) {
    const container = useRef<HTMLDivElement>(null);
    const portal = useRef<HTMLDivElement>(null);
    const sections: FlipSection[] = [{ name: 'current', ids }];
    useFlipTransitions(container, portal, sections, undefined, [open]);
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

function fakeAnimate(this: Element, keyframes: Keyframe[]): Animation {
    animations.push({ target: this, keyframes });
    return {
        finished: new Promise<void>(() => {}),
        startTime: null,
        pause() {},
        play() {},
        cancel() {},
    } as unknown as Animation;
}

function maskReach(): unknown[] {
    const mask = animations.find(a => a.target.classList.contains('flip-disclosure-mask'));
    expect(mask, 'the disclosure mounts a mask').toBeDefined();
    return mask!.keyframes.map(k => k[FADE_REACH]);
}

describe('the drawer disclosure fade', () => {
    beforeEach(() => {
        animations = [];
        host = document.createElement('div');
        document.body.appendChild(host);
        vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(rowRect);
        Element.prototype.animate = fakeAnimate as typeof Element.prototype.animate;
        vi.stubGlobal('requestAnimationFrame', () => 0);
    });

    afterEach(() => {
        render(null, host);
        host.remove();
        vi.restoreAllMocks();
        vi.unstubAllGlobals();
    });

    it('deepens as the rows roll up under their parent', () => {
        render(<Drawer ids={['a', 'b', 'c']} open />, host);
        render(<Drawer ids={['a']} open={false} />, host);
        expect(maskReach()).toEqual(['0px', '100px']);
    });

    it('drains as the rows roll down into place', () => {
        render(<Drawer ids={['a']} open={false} />, host);
        render(<Drawer ids={['a', 'b', 'c']} open />, host);
        expect(maskReach()).toEqual(['100px', '0px']);
    });
});
