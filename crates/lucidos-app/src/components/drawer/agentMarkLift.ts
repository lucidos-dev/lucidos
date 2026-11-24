import { getRemPx } from '../../utils/dom';


const AGENT_LIFT_CLEARANCE_REM = 0.5;

export interface Box {
    left: number;
    right: number;
    top: number;
    bottom: number;
}

export function titleLines(rects: readonly Box[]): Box[] {
    const byCentre = [...rects].sort((a, b) => a.top + a.bottom - (b.top + b.bottom));
    const lines: Box[] = [];
    for (const r of byCentre) {
        const line = lines[lines.length - 1];
        if (line && (r.top + r.bottom) / 2 < line.bottom) {
            line.left = Math.min(line.left, r.left);
            line.right = Math.max(line.right, r.right);
            line.top = Math.min(line.top, r.top);
            line.bottom = Math.max(line.bottom, r.bottom);
        } else {
            lines.push({ left: r.left, right: r.right, top: r.top, bottom: r.bottom });
        }
    }
    return lines;
}

export function stackLiftTarget(
    rects: readonly Box[], actions: Box | null, bottom: Box, stackGap: number, gap: number,
    metaItems: readonly Box[] = [],
): number | null {
    const height = bottom.bottom - bottom.top;
    const top = bottom.top - stackGap - height;
    const end = bottom.top - stackGap;
    const overlaps = (r: Box, clearance: number) => r.right > bottom.left - clearance
        && r.left < bottom.right + clearance && r.bottom > top - clearance && r.top < end + clearance;
    if (titleLines(rects).some(r => overlaps(r, gap)) || metaItems.some(r => overlaps(r, 0))
        || (actions && actions.right > actions.left && overlaps(actions, gap))) return null;
    return (top + end) / 2;
}

const AGENT_LIFT_VAR = '--thread-row-agent-lift';

export function clearStackedMark(left: HTMLElement) {
    const meta = left.querySelector<HTMLElement>('.thread-row-meta');
    meta?.removeAttribute('data-agent-lift');
    meta?.style.removeProperty(AGENT_LIFT_VAR);
}

export function placeStackedMark(left: HTMLElement) {
    const meta = left.querySelector<HTMLElement>('.thread-row-meta');
    const title = left.querySelector<HTMLElement>('.thread-row-title-text');
    const mark = left.querySelector<HTMLElement>('.thread-row-agents > .thread-row-agent:last-child');
    if (!meta || !title || !mark) {
        clearStackedMark(left);
        return;
    }
    const scale = left.offsetWidth > 0 ? left.getBoundingClientRect().width / left.offsetWidth : 1;
    const range = document.createRange();
    range.selectNodeContents(title);
    const rects = Array.from(range.getClientRects()).filter(r => r.width > 0 || r.height > 0);
    const actions = left.querySelector<HTMLElement>('.thread-row-actions-line')?.getBoundingClientRect() ?? null;
    const gap = AGENT_LIFT_CLEARANCE_REM * getRemPx() * scale;
    meta.setAttribute('data-agent-lift', '');
    const agents = mark.parentElement!;
    const stackGap = parseFloat(getComputedStyle(agents).rowGap) * scale;
    const metaItems = Array.from(meta.children)
        .filter(item => item !== agents)
        .map(item => item.getBoundingClientRect());
    const centre = stackLiftTarget(rects, actions, mark.getBoundingClientRect(), stackGap, gap, metaItems);
    if (centre === null) {
        clearStackedMark(left);
        return;
    }
    const lift = (meta.getBoundingClientRect().top - centre) / scale / getRemPx();
    meta.style.setProperty(AGENT_LIFT_VAR, `${lift}rem`);
}
