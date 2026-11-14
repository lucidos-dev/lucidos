/** The drawer's *row window*: which rows of a section are in the DOM, and
 *  what stands in for the rest (docs/glossary.md § Row window, ADR 0431).
 *  Pure, so the rules below are unit-testable without layout.
 *
 *  A gap stands in for a run of unrendered rows, and its height is the sum of
 *  theirs. Above the viewport that sum is always measured, never estimated, so
 *  moving the window never moves a row. An unknown row above a span is
 *  rendered instead, and a later plan folds it once it has been measured. */

/** A vertical range, in px. */
export interface Span { top: number; bottom: number }

/** A run of rows `[from, to)`, rendered, or folded into one gap. */
export type RowSegment =
    | { kind: 'rows'; from: number; to: number }
    | { kind: 'gap'; from: number; to: number; height: number };

export interface RowPlanInput {
    count: number;
    /** Row `i`'s measured height, or undefined when unknown or stale. */
    heightAt: (i: number) => number | undefined;
    /** The height assumed for an unknown row below every span. */
    estimate: number;
    /** Ranges, in the section's own coordinates, whose rows must render. */
    spans: readonly Span[];
    /** Rows rendered wherever they sit. */
    pinned: ReadonlySet<number>;
}

export function planRowWindow({ count, heightAt, estimate, spans, pinned }: RowPlanInput): RowSegment[] {
    const rendered = new Array<boolean>(count).fill(false);
    // An unknown row starting above the lowest span's top would be an estimate
    // above the viewport, so it renders.
    const lowestTop = spans.reduce((max, s) => Math.max(max, s.top), -Infinity);
    let top = 0;
    for (let i = 0; i < count; i++) {
        const known = heightAt(i);
        const bottom = top + (known ?? estimate);
        if (spans.some(s => bottom > s.top && top < s.bottom)) rendered[i] = true;
        else if (known === undefined && top < lowestTop) rendered[i] = true;
        top = bottom;
    }
    for (const i of pinned) if (i >= 0 && i < count) rendered[i] = true;

    const segments: RowSegment[] = [];
    let from = 0;
    while (from < count) {
        const kind = rendered[from];
        let to = from;
        let height = 0;
        while (to < count && rendered[to] === kind) {
            if (!kind) height += heightAt(to) ?? estimate;
            to++;
        }
        segments.push(kind ? { kind: 'rows', from, to } : { kind: 'gap', from, to, height });
        from = to;
    }
    return segments;
}

/** A plan's identity: two plans with the same key render the same DOM. */
export function segmentsKey(segments: readonly RowSegment[]): string {
    return segments.map(s => (s.kind === 'rows' ? `r${s.from}-${s.to}` : `g${s.from}-${s.to}:${s.height}`)).join(',');
}

/** Below this, a height change is layout rounding, not a resize. */
const RESIZE_EPSILON_PX = 0.5;

/** What recording a height found. `environment` means a row resized while its
 *  content stayed the same: the width, a font or the UI scale changed. */
export type RecordOutcome = 'new' | 'same' | 'environment';

interface Measured { height: number; key: string; generation: number }

/** Measured row heights, each valid only under the layout key it was measured
 *  with and the environment it was measured in. */
export class RowHeights {
    private byId = new Map<string, Measured>();
    private current = 0;
    private environment: string | null = null;
    /** Sum and count of the heights in the current generation, for `estimate`. */
    private sum = 0;
    private count = 0;

    /** Bumped each time every height goes stale. */
    get generation(): number {
        return this.current;
    }

    valid(id: string, key: string): number | undefined {
        const m = this.byId.get(id);
        return m && m.key === key && m.generation === this.current ? m.height : undefined;
    }

    record(id: string, key: string, height: number): RecordOutcome {
        const prev = this.byId.get(id);
        const prevCounts = prev?.generation === this.current;
        if (prevCounts) this.sum -= prev.height;
        else this.count++;
        this.sum += height;
        this.byId.set(id, { height, key, generation: this.current });
        if (!prev || prev.key !== key || !prevCounts) return 'new';
        return Math.abs(prev.height - height) > RESIZE_EPSILON_PX ? 'environment' : 'same';
    }

    /** Every height goes stale: the environment they were measured in is gone. */
    invalidate(): void {
        this.current++;
        this.sum = 0;
        this.count = 0;
    }

    /** Name the environment heights are measured in (a width and a root font
     *  size). Naming a different one than last time invalidates every height
     *  and answers true: it changed while no list was watching. */
    useEnvironment(environment: string): boolean {
        if (this.environment === environment) return false;
        const changed = this.environment !== null;
        this.environment = environment;
        if (changed) this.invalidate();
        return changed;
    }

    /** The mean measured height in the current environment, or `fallback`. */
    estimate(fallback: number): number {
        return this.count > 0 ? this.sum / this.count : fallback;
    }
}

/** The span a window renders around `viewport`: two viewports either side. */
export function renderSpanFor(viewport: Span): Span {
    const vh = viewport.bottom - viewport.top;
    return { top: viewport.top - 2 * vh, bottom: viewport.bottom + 2 * vh };
}

/** Whether `span` no longer keeps a viewport of margin either side of
 *  `viewport`. Re-planning only then is what re-renders the window about once
 *  per viewport scrolled, rather than on every scroll event. */
export function needsNewSpan(span: Span | null, viewport: Span): boolean {
    if (!span) return true;
    const vh = viewport.bottom - viewport.top;
    return span.top > viewport.top - vh || span.bottom < viewport.bottom + vh;
}
