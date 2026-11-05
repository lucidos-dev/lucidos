import { getRemPx } from '../../utils/dom';

/** Where a two-backend row puts its top agent mark. The marks stack in the
 *  mark column. A wrapped title may end left of that column. The top mark
 *  then rises onto the title's last line, so the row is no taller than a
 *  one-backend row. `liftTarget` is the pure geometry. `placeStackedMark` reads
 *  the DOM and writes the result back to it. */

/** How far the lifted mark stays from the title's last line, in rem. */
const AGENT_LIFT_CLEARANCE_REM = 0.5;

export interface Box {
    left: number;
    right: number;
    top: number;
    bottom: number;
}

/** The title's line boxes, one per rendered line, top to bottom. Rects from
 *  inline pieces (text, icons) join a line while their centre sits above the
 *  line's bottom edge. */
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

/** The vertical centre of the title's last line, when the top mark may rise
 *  onto it, else `null`. The last line must end at least `gap` left of the
 *  column. The row's actions must not sit on that line at the column. A
 *  one-line title on desktop never qualifies, since the pin and ⋯
 *  own that line's end. */
export function liftTarget(
    rects: readonly Box[],
    actions: Box | null,
    columnLeft: number,
    gap: number,
): number | null {
    const last = titleLines(rects).pop();
    if (!last || last.right > columnLeft - gap) return null;
    const actionsOnLine = !!actions
        && actions.right > actions.left
        && actions.bottom > last.top
        && actions.top < last.bottom
        && actions.right > columnLeft - gap;
    return actionsOnLine ? null : (last.top + last.bottom) / 2;
}

const AGENT_LIFT_VAR = '--thread-row-agent-lift';

/** Drops the lift `placeStackedMark` set, so the marks stack again. */
export function clearStackedMark(left: HTMLElement) {
    const meta = left.querySelector<HTMLElement>('.thread-row-meta');
    meta?.removeAttribute('data-agent-lift');
    meta?.style.removeProperty(AGENT_LIFT_VAR);
}

/** Marks a stacked row's meta line with `data-agent-lift` when its top mark
 *  may rise onto the title's last line (`liftTarget`). Writes the rise as a
 *  length in `--thread-row-agent-lift`. Reads the rendered line boxes,
 *  un-scaled by the row's own FLIP scale like `ContextChip`. Writes the DOM
 *  directly: the result is paint, so it must not re-render the memoised row. */
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
    const centre = liftTarget(rects, actions, mark.getBoundingClientRect().left, gap);
    if (centre === null) {
        clearStackedMark(left);
        return;
    }
    const lift = (meta.getBoundingClientRect().top - centre) / scale / getRemPx();
    meta.setAttribute('data-agent-lift', '');
    meta.style.setProperty(AGENT_LIFT_VAR, `${lift}rem`);
}
