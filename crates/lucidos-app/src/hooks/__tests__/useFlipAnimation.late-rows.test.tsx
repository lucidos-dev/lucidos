// @vitest-environment jsdom
/**
 * The drawer windows its rows, so a row off screen stands in a gap. Collapsing
 * Current lifts Archive into view, and its rows mount one commit later, after
 * the FLIP pass has run. They must move with the header above them, not
 * appear at their final place over the rows still rolling away.
 */
import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import { render } from 'preact';
import { useRef } from 'preact/hooks';
import { signal } from '@preact/signals';
import { useFlipTransitions, type FlipSection } from '../useFlipAnimation';

const ROW_HEIGHT = 50;
const CURRENT = ['c1', 'c2', 'c3'];
const ARCHIVE = ['x', 'y'];
/** What the fake timeline reads when the batch's first frame starts it. */
const FIRST_FRAME = 1234;

interface FakeAnimation {
    target: Element;
    keyframes: Keyframe[];
    calls: string[];
    startTime: number | null;
    playState: AnimationPlayState;
}

let host: HTMLElement;
let animations: FakeAnimation[];
/** The archived rows the row window has mounted. */
const mountedArchive = signal<string[]>([]);

function ArchiveRows() {
    return <>{mountedArchive.value.map(id => <div key={id} data-flip-id={id}>{id}</div>)}</>;
}

interface DrawerProps {
    collapsed?: boolean;
    current?: string[];
    archive?: string[];
    resetKey?: unknown;
}

function Drawer({ collapsed = false, current = CURRENT, archive = ARCHIVE, resetKey }: DrawerProps) {
    const container = useRef<HTMLDivElement>(null);
    const shown = collapsed ? [] : current;
    const sections: FlipSection[] = [
        { name: 'current', ids: ['__section_current', ...shown] },
        { name: 'archive', ids: ['__section_archive', ...archive] },
    ];
    useFlipTransitions(container, sections, resetKey, [collapsed]);
    return (
        <div ref={container}>
            {['__section_current', ...shown, '__section_archive'].map(id => <div key={id} data-flip-id={id}>{id}</div>)}
            <ArchiveRows />
        </div>
    );
}

/** A row sits at its index in the list. Anything else has no box. */
function rowRect(this: HTMLElement): DOMRect {
    const id = this.dataset.flipId;
    const rows = id && this.parentElement ? [...this.parentElement.querySelectorAll<HTMLElement>('[data-flip-id]')] : [];
    const top = id ? rows.indexOf(this) * ROW_HEIGHT : 0;
    const height = id ? ROW_HEIGHT : 0;
    return { top, left: 0, width: id ? 300 : 0, height, bottom: top + height, right: 300, x: 0, y: top, toJSON: () => ({}) } as DOMRect;
}

function fakeAnimate(this: Element, keyframes: Keyframe[]): Animation {
    const anim: FakeAnimation = { target: this, keyframes, calls: [], startTime: null, playState: 'running' };
    animations.push(anim);
    return {
        finished: new Promise<void>(() => {}),
        get playState() { return anim.playState; },
        get startTime() { return anim.startTime; },
        set startTime(time: number | null) {
            anim.startTime = time;
            anim.playState = 'running';
            anim.calls.push('start');
        },
        pause() {
            anim.playState = 'paused';
            anim.calls.push('pause');
        },
        play() {
            anim.playState = 'running';
            anim.calls.push('play');
        },
        cancel() {},
    } as unknown as Animation;
}

function rowOf(id: string): HTMLElement {
    return host.querySelector<HTMLElement>(`[data-flip-id="${id}"]`)!;
}

/** The animations run on the row itself, never on a copy of it. */
function animationsOn(id: string): FakeAnimation[] {
    const row = rowOf(id);
    return animations.filter(a => a.target === row);
}

function keyframesOf(id: string): Keyframe[][] {
    return animationsOn(id).map(a => a.keyframes);
}

async function settle(): Promise<void> {
    await new Promise(resolve => setTimeout(resolve, 0));
}

describe('rows that mount a commit after the FLIP pass', () => {
    beforeEach(() => {
        animations = [];
        mountedArchive.value = [];
        host = document.createElement('div');
        document.body.appendChild(host);
        vi.spyOn(HTMLElement.prototype, 'getBoundingClientRect').mockImplementation(rowRect);
        Element.prototype.animate = fakeAnimate as typeof Element.prototype.animate;
        vi.stubGlobal('requestAnimationFrame', (cb: FrameRequestCallback) => setTimeout(() => cb(0), 0));
        Object.defineProperty(document, 'timeline', { value: { currentTime: FIRST_FRAME }, configurable: true });
    });

    afterEach(() => {
        render(null, host);
        host.remove();
        vi.restoreAllMocks();
        vi.unstubAllGlobals();
        delete (document as { timeline?: unknown }).timeline;
    });

    it('slide with the header above them when they mount before the first frame', async () => {
        render(<Drawer />, host);
        render(<Drawer collapsed />, host);
        const header = keyframesOf('__section_archive');
        expect(header, 'the Archive header slides up').toHaveLength(1);

        // The row window mounts the archived rows in its own follow-up commit.
        mountedArchive.value = ARCHIVE;
        await settle();

        for (const id of ARCHIVE) {
            expect(keyframesOf(id), `${id} moves with the header`).toEqual(header);
            expect(rowOf(id).classList.contains('flip-animating')).toBe(true);
        }
    });

    it('slide with the header when they mount after the batch has started', async () => {
        render(<Drawer />, host);
        render(<Drawer collapsed />, host);
        await settle();

        mountedArchive.value = ARCHIVE;
        await settle();

        for (const id of ARCHIVE) expect(keyframesOf(id), id).toEqual(keyframesOf('__section_archive'));
    });

    it('wait for the batch\'s first frame when they mount before it', async () => {
        render(<Drawer />, host);
        render(<Drawer collapsed />, host);
        mountedArchive.value = ARCHIVE;
        await settle();

        for (const id of ARCHIVE) {
            const [anim] = animationsOn(id);
            expect(anim.calls[0], `${id} never plays ahead of the batch`).toBe('pause');
            expect(anim.startTime, id).toBe(FIRST_FRAME);
        }
    });

    it('take the batch\'s start time when they mount after its first frame', async () => {
        render(<Drawer />, host);
        render(<Drawer collapsed />, host);
        await settle();

        mountedArchive.value = ARCHIVE;
        await settle();

        for (const id of ARCHIVE) {
            const [anim] = animationsOn(id);
            expect(anim.calls, id).toEqual(['start']);
            expect(anim.startTime, id).toBe(FIRST_FRAME);
        }
    });

    it('fade in at their new place when the thread changed sections', async () => {
        mountedArchive.value = ARCHIVE;
        render(<Drawer />, host);
        await settle();

        // c1 is archived. The row window brings its new row into view a commit later.
        render(<Drawer current={['c2', 'c3']} archive={['c1', ...ARCHIVE]} />, host);
        mountedArchive.value = ['c1', ...ARCHIVE];
        await settle();

        expect(keyframesOf('c1')).toEqual([[{ opacity: '0' }, { opacity: '1' }]]);
    });

    it('stay put when a reset lands while the batch runs', async () => {
        render(<Drawer />, host);
        render(<Drawer collapsed />, host);
        render(<Drawer collapsed resetKey="other filter" />, host);

        mountedArchive.value = ARCHIVE;
        await settle();

        for (const id of ARCHIVE) expect(keyframesOf(id), id).toEqual([]);
    });

    it('stay put when they mount with no batch running', async () => {
        render(<Drawer />, host);
        mountedArchive.value = ARCHIVE;
        await settle();

        for (const id of ARCHIVE) expect(keyframesOf(id), id).toEqual([]);
    });
});
