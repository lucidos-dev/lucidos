
export const EASING_DISCLOSURE = 'cubic-bezier(0.4, 0, 0.2, 1)';

export const FADE_REACH = '--disclosure-fade-reach';

const DISCLOSURE_PX_PER_SEC = 1200;
export const DISCLOSURE_MIN_MS = 260;
export const DISCLOSURE_MAX_MS = 420;

export function disclosureDurationMs(distance: number): number {
    return Math.min(DISCLOSURE_MAX_MS, Math.max(DISCLOSURE_MIN_MS, distance / DISCLOSURE_PX_PER_SEC * 1000));
}

export function rollCap(height: number, line: number, viewportHeight: number): number {
    return Math.max(0, Math.min(height, viewportHeight - line));
}
