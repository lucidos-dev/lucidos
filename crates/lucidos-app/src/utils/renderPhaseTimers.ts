
let markdownParseMsTotal = 0;
let linkifyMsTotal = 0;

export function addMarkdownParseMs(ms: number): void {
  markdownParseMsTotal += ms;
}

export function addLinkifyMs(ms: number): void {
  linkifyMsTotal += ms;
}

export interface RenderPhaseTotals {
  markdownMs: number;
  linkifyMs: number;
}

export function readRenderPhaseTotals(): RenderPhaseTotals {
  return { markdownMs: markdownParseMsTotal, linkifyMs: linkifyMsTotal };
}

export interface PerfBaseline {
  start: number;
  md: number;
  link: number;
}

export function currentPerfBaseline(): PerfBaseline {
  return { start: performance.now(), md: markdownParseMsTotal, link: linkifyMsTotal };
}

export function _resetRenderPhaseTimersForTesting(): void {
  markdownParseMsTotal = 0;
  linkifyMsTotal = 0;
}
