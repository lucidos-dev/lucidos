
export interface Span { top: number; bottom: number }

export type RowSegment =
    | { kind: 'rows'; from: number; to: number }
    | { kind: 'gap'; from: number; to: number; height: number };

export interface RowPlanInput {
    count: number;
    heightAt: (i: number) => number | undefined;
    estimate: number;
    spans: readonly Span[];
    pinned: ReadonlySet<number>;
}

export function planRowWindow({ count, heightAt, estimate, spans, pinned }: RowPlanInput): RowSegment[] {
    const rendered = new Array<boolean>(count).fill(false);
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

export function segmentsKey(segments: readonly RowSegment[]): string {
    return segments.map(s => (s.kind === 'rows' ? `r${s.from}-${s.to}` : `g${s.from}-${s.to}:${s.height}`)).join(',');
}

const RESIZE_EPSILON_PX = 0.5;

export type RecordOutcome = 'new' | 'same' | 'environment';

interface Measured { height: number; key: string; generation: number }

export class RowHeights {
    private byId = new Map<string, Measured>();
    private current = 0;
    private environment: string | null = null;
    private sum = 0;
    private count = 0;

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

    invalidate(): void {
        this.current++;
        this.sum = 0;
        this.count = 0;
    }

    useEnvironment(environment: string): boolean {
        if (this.environment === environment) return false;
        const changed = this.environment !== null;
        this.environment = environment;
        if (changed) this.invalidate();
        return changed;
    }

    estimate(fallback: number): number {
        return this.count > 0 ? this.sum / this.count : fallback;
    }
}

export function renderSpanFor(viewport: Span): Span {
    const vh = viewport.bottom - viewport.top;
    return { top: viewport.top - 2 * vh, bottom: viewport.bottom + 2 * vh };
}

export function needsNewSpan(span: Span | null, viewport: Span): boolean {
    if (!span) return true;
    const vh = viewport.bottom - viewport.top;
    return span.top > viewport.top - vh || span.bottom < viewport.bottom + vh;
}
