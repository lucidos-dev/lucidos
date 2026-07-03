import { Fragment } from 'preact';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { marked } from 'marked';
import type { Token, Tokens } from 'marked';
// escapeHtmlAttr is a DOM-free escaper; importing it also runs markedConfig's
// marked.use(...) side effects (previously a bare side-effect import).
import { escapeHtmlAttr } from '../../utils/markedConfig';
import type { DiffFile } from '../../store/store';
import { getChangeFileContent, getRepoFileContent } from '../../api/client';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';

interface Props {
  file: DiffFile;
  /** Lucidos change ID — fetch via /api/v1/changes/:id/file. Null for external-repo
   *  Claude Code sessions, which have no Change row; in that case `gitRef` carries the
   *  worktree branch and we fetch via /api/v1/repositories/:id/file?ref=. */
  changeId: string | null;
  repoId: string;
  gitRef: string | null;
}

type BlockStatus = 'unchanged' | 'added' | 'changed';

interface LineRun { start: number; end: number }

/** A consecutive block of removed lines, anchored to the new-file line it sat
 *  before. `anchor` is the new-file line number the removed block precedes — the
 *  block renders just above the rendered content at that position. */
export interface DeletionRun { anchor: number; lines: string[] }

const STRIP_LIST_OPEN = /^\s*<(ul|ol)[^>]*>/;
const STRIP_LIST_CLOSE = /<\/(ul|ol)>\s*$/;
const LI_OPEN_TAG = /^<li(\s[^>]*)?>/;
const LI_EXISTING_CLASS = /^<li([^>]*?)\sclass="([^"]*)"/;

export function additionRuns(file: DiffFile): LineRun[] {
  const runs: LineRun[] = [];
  for (const hunk of file.hunks) {
    let newLine = hunk.new_start;
    let runStart: number | null = null;
    for (const line of hunk.lines) {
      if (line.type === 'addition') {
        if (runStart === null) runStart = newLine;
        newLine++;
      } else {
        if (runStart !== null) {
          runs.push({ start: runStart, end: newLine - 1 });
          runStart = null;
        }
        if (line.type === 'context') newLine++;
      }
    }
    if (runStart !== null) runs.push({ start: runStart, end: newLine - 1 });
  }
  return runs;
}

/** The new-file line ranges the hunks span — changed lines plus the context
 *  lines git already included around each hunk. Used to scope the rendered diff
 *  to just the changed regions (mirroring the raw `DiffView`, which only shows
 *  hunks); content outside any hunk is collapsed into a gap marker. The "Show
 *  full file" toggle is the way to see the whole document. */
export function hunkCoverage(file: DiffFile): LineRun[] {
  const runs: LineRun[] = [];
  for (const hunk of file.hunks) {
    // new_count counts context + additions (deletions occupy no new-file line),
    // so [new_start, new_start + new_count - 1] is the hunk's new-file extent.
    if (hunk.new_count > 0) {
      runs.push({ start: hunk.new_start, end: hunk.new_start + hunk.new_count - 1 });
    }
  }
  return runs;
}

function classifyRange(start: number, end: number, runs: LineRun[]): BlockStatus {
  if (end < start) return 'unchanged';
  let overlap = 0;
  for (const r of runs) {
    if (r.end < start || r.start > end) continue;
    overlap += Math.min(r.end, end) - Math.max(r.start, start) + 1;
  }
  if (overlap === 0) return 'unchanged';
  if (overlap >= end - start + 1) return 'added';
  return 'changed';
}

function rawNewlines(raw: string): number {
  let n = 0;
  for (let i = 0; i < raw.length; i++) if (raw.charCodeAt(i) === 10) n++;
  return n;
}

// A token's first character lives on `start`. The cursor advances by the number
// of `\n`s in its raw — that's what positions the next token. Lines the token
// visually OCCUPIES are [start, start + max(0, newlines - 1)]; trailing blank
// lines bundled into the raw don't count as content.
function tokenEndLine(start: number, newlines: number): number {
  return start + Math.max(0, newlines - 1);
}

function statusClass(s: BlockStatus): string {
  return s === 'added' ? 'diff-rendered-added'
    : s === 'changed' ? 'diff-rendered-changed'
    : '';
}

function isList(t: Token): t is Tokens.List { return t.type === 'list'; }

function addLiClass(html: string, cls: string): string {
  // Merge into an existing class attribute (e.g. marked's task-list-item) so
  // we don't emit a duplicate class= attribute.
  if (LI_EXISTING_CLASS.test(html)) {
    return html.replace(LI_EXISTING_CLASS, `<li$1 class="$2 ${cls}"`);
  }
  return html.replace(LI_OPEN_TAG, (_m, attrs) => `<li class="${cls}"${attrs ?? ''}>`);
}

function renderListWithItemMarking(list: Tokens.List, listStartLine: number, runs: LineRun[]): string {
  const tag = list.ordered ? 'ol' : 'ul';
  const startAttr = list.ordered && list.start !== 1 && typeof list.start === 'number'
    ? ` start="${list.start}"`
    : '';

  let line = listStartLine;
  const lis: string[] = [];

  for (const item of list.items) {
    const newlines = rawNewlines(item.raw);
    const start = line;
    const end = tokenEndLine(start, newlines);
    const status = classifyRange(start, end, runs);

    // Wrap each item in a synthetic single-item list so marked's renderer handles
    // task lists, loose-vs-tight, and nested content correctly. Strip the outer
    // ul/ol — we control marked's deterministic output, not user HTML.
    const synthetic: Tokens.List = { ...list, items: [item], raw: item.raw };
    const itemHtml = marked.parser([synthetic]);
    const inner = itemHtml.replace(STRIP_LIST_OPEN, '').replace(STRIP_LIST_CLOSE, '').trim();
    const cls = statusClass(status);
    lis.push(cls ? addLiClass(inner, cls) : inner);

    line += newlines;
  }

  return `<${tag}${startAttr}>${lis.join('')}</${tag}>`;
}

// A removed block is raw old-file content (not rendered as markdown — it no
// longer exists in the new file). It's a top-level sibling, so the change-strip
// overlay can find it by class and paint a red bar/tint at the panel edge.
function renderRemovedBlock(lines: string[]): string {
  const body = lines.map(escapeHtmlAttr).join('\n');
  return `<div class="diff-rendered-removed"><pre class="diff-removed-lines">${body}</pre></div>`;
}

/** Renders the post-change markdown with change marks. When `coverage` is
 *  provided (the hunks' new-file extents), only tokens overlapping a covered
 *  range are rendered — the rest of the document is collapsed into a single gap
 *  marker per omitted run, so the rich diff shows just the changes rather than
 *  the whole file. Pass `null` (the default) to render the entire document. */
export function renderDiffMarked(
  content: string,
  runs: LineRun[],
  dels: DeletionRun[] = [],
  coverage: LineRun[] | null = null,
): string {
  const tokens = marked.lexer(content);
  const sortedDels = [...dels].sort((a, b) => a.anchor - b.anchor);
  const parts: string[] = [];
  let line = 1;
  let di = 0;
  // A non-space token outside `coverage` was skipped since the last rendered
  // part — flushed as one gap marker before the next rendered part.
  let pendingGap = false;

  const isCovered = (start: number, end: number): boolean => {
    if (!coverage) return true;
    return coverage.some(c => !(c.end < start || c.start > end));
  };

  const flushGap = () => {
    if (pendingGap) {
      parts.push('<div class="diff-rendered-gap" aria-hidden="true">⋯</div>');
      pendingGap = false;
    }
  };

  // Flush every removed block anchored before `lineExclusive`, so a deletion
  // renders just above the first token at-or-after its original position. A
  // deletion is rendered content, so it closes any pending gap first.
  const flushDelsBefore = (lineExclusive: number) => {
    while (di < sortedDels.length && sortedDels[di].anchor < lineExclusive) {
      flushGap();
      parts.push(renderRemovedBlock(sortedDels[di].lines));
      di++;
    }
  };

  for (const tok of tokens) {
    const newlines = rawNewlines(tok.raw ?? '');
    const start = line;
    const end = tokenEndLine(start, newlines);
    flushDelsBefore(start + 1);

    // Blank-line `space` tokens are never content and never a gap on their own.
    if (tok.type === 'space') {
      line += newlines;
      continue;
    }

    // Outside the hunk coverage → omit, marking a gap for the next flush.
    if (!isCovered(start, end)) {
      pendingGap = true;
      line += newlines;
      continue;
    }

    flushGap();
    if (isList(tok)) {
      // Lists are marked per-item; every other block (paragraph, heading,
      // blockquote, code) renders whole and is marked as a unit.
      const html = renderListWithItemMarking(tok, start, runs);
      const status = classifyRange(start, end, runs);
      const wrapClass = status === 'added' ? 'diff-rendered-block-added' : '';
      parts.push(wrapClass ? `<div class="${wrapClass}">${html}</div>` : html);
    } else {
      const status = classifyRange(start, end, runs);
      const html = marked.parser([tok]);
      const cls = statusClass(status);
      parts.push(cls ? `<div class="${cls}">${html}</div>` : html);
    }

    line += newlines;
  }

  // Deletions past the last token (e.g. trailing lines removed at EOF).
  flushDelsBefore(Infinity);
  // Trailing omitted content below the last change.
  flushGap();

  return parts.join('');
}

export function deletionRuns(file: DiffFile): DeletionRun[] {
  const runs: DeletionRun[] = [];
  for (const hunk of file.hunks) {
    let newLine = hunk.new_start;
    let current: string[] | null = null;
    let anchor = newLine;
    for (const line of hunk.lines) {
      if (line.type === 'deletion') {
        if (current === null) { current = []; anchor = newLine; }
        current.push(line.content);
      } else {
        if (current !== null) { runs.push({ anchor, lines: current }); current = null; }
        // context + addition both advance the new-file line counter; a deletion
        // does not (it has no line in the new file).
        newLine++;
      }
    }
    if (current !== null) runs.push({ anchor, lines: current });
  }
  return runs;
}

interface Strip { top: number; height: number; variant: 'added' | 'changed' | 'removed' }

export function RenderedDiff({ file, changeId, repoId, gitRef }: Props) {
  const [content, setContent] = useState<Loadable<string>>({ status: 'not-loaded' });
  const showLoading = useDelayedLoading(content);
  const containerRef = useRef<HTMLDivElement>(null);
  const contentRef = useRef<HTMLDivElement>(null);
  const [strips, setStrips] = useState<Strip[]>([]);

  useEffect(() => {
    let canceled = false;
    setContent({ status: 'loading' });
    const fetchAfter = changeId
      ? getChangeFileContent(changeId, file.path)
      : getRepoFileContent(repoId, file.path, gitRef ?? undefined);
    fetchAfter
      .then(text => { if (!canceled) setContent({ status: 'loaded', data: text }); })
      .catch((e: unknown) => { if (!canceled) setContent(toFailed(e)); });
    return () => { canceled = true; };
  }, [changeId, repoId, gitRef, file.path]);

  const runs = useMemo(() => additionRuns(file), [file]);
  const dels = useMemo(() => deletionRuns(file), [file]);
  // Scope the rendered diff to the hunks (changed lines + git's context) — the
  // "Show full file" toggle is the way to see the whole document.
  const coverage = useMemo(() => hunkCoverage(file), [file]);

  const html = useMemo(() => {
    if (content.status !== 'loaded') return null;
    return renderDiffMarked(content.data, runs, dels, coverage);
  }, [content, runs, dels, coverage]);

  // Strips render in a separate overlay layer so they reach the panel edge
  // regardless of where the marked element sits in the indentation hierarchy
  // (paragraphs, blockquotes, nested <li>). Border-on-element approaches break
  // for nested <li> because the negative margin can't escape the <ul> padding.
  useLayoutEffect(() => {
    const container = containerRef.current;
    const contentEl = contentRef.current;
    if (!container || !contentEl) return;

    const recompute = () => {
      const containerRect = container.getBoundingClientRect();
      const raw: Strip[] = [];
      const markedEls = contentEl.querySelectorAll<HTMLElement>(
        '.diff-rendered-added, .diff-rendered-changed, .diff-rendered-block-added, .diff-rendered-removed',
      );
      for (const el of Array.from(markedEls)) {
        const r = el.getBoundingClientRect();
        raw.push({
          top: r.top - containerRect.top + container.scrollTop,
          height: r.height,
          variant: el.classList.contains('diff-rendered-removed') ? 'removed'
            : el.classList.contains('diff-rendered-changed') ? 'changed'
            : 'added',
        });
      }
      // Merge strips that touch or are nearly adjacent. Without this, paragraph /
      // list-item margins (typically 4–8px) leave visible white bands between
      // strips for adjacent marked siblings, and sub-element marking (e.g. each
      // nested <li> in an added block) renders as 3 separated strips instead of
      // one continuous bar.
      raw.sort((a, b) => a.top - b.top);
      const merged: Strip[] = [];
      const MERGE_GAP = 16;
      for (const s of raw) {
        const prev = merged[merged.length - 1];
        if (prev && prev.variant === s.variant && s.top <= prev.top + prev.height + MERGE_GAP) {
          prev.height = Math.max(prev.top + prev.height, s.top + s.height) - prev.top;
        } else {
          merged.push({ ...s });
        }
      }
      setStrips(merged);
    };

    recompute();
    const ro = new ResizeObserver(recompute);
    ro.observe(contentEl);
    return () => ro.disconnect();
  }, [html]);

  if (content.status === 'failed') {
    return <LoadableError noun="diff" error={content.error} />;
  }
  if (content.status !== 'loaded') {
    if (!showLoading) return null;
    return <div class="loading-spinner" />;
  }

  return (
    <div class="rendered-diff" ref={containerRef}>
      <div class="diff-strip-layer" aria-hidden="true">
        {strips.map((s, i) => {
          const style = `top:${s.top}px;height:${s.height}px`;
          return (
            <Fragment key={i}>
              <div class={`diff-bg diff-bg-${s.variant}`} style={style} />
              <div class={`diff-strip diff-strip-${s.variant}`} style={style} />
            </Fragment>
          );
        })}
      </div>
      <div class="response-content markdown-content" ref={contentRef} dangerouslySetInnerHTML={{ __html: html ?? '' }} />
    </div>
  );
}
