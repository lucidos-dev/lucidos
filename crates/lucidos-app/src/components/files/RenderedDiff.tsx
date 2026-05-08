import { Fragment } from 'preact';
import { useEffect, useLayoutEffect, useMemo, useRef, useState } from 'preact/hooks';
import { marked } from 'marked';
import type { Token, Tokens } from 'marked';
import '../../utils/markedConfig';
import type { DiffFile } from '../../store/store';
import { getChangeFileContent, getRepoFileContent } from '../../api/client';
import { escapeHtml } from '../../utils/escapeHtml';
import type { Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';

interface Props {
  file: DiffFile;
  /** Lucidos change ID — fetch via /api/changes/:id/file. Null for external-repo
   *  CC sessions, which have no Change row; in that case `gitRef` carries the
   *  worktree branch and we fetch via /api/repositories/:id/file?ref=. */
  changeId: string | null;
  repoId: string;
  gitRef: string | null;
}

type BlockStatus = 'unchanged' | 'added' | 'changed';

interface LineRun { start: number; end: number }

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
function isBlockquote(t: Token): t is Tokens.Blockquote { return t.type === 'blockquote'; }

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

export function renderDiffMarked(content: string, runs: LineRun[]): string {
  const tokens = marked.lexer(content);
  const parts: string[] = [];
  let line = 1;

  for (const tok of tokens) {
    const newlines = rawNewlines(tok.raw ?? '');
    const start = line;
    const end = tokenEndLine(start, newlines);

    if (isList(tok)) {
      const html = renderListWithItemMarking(tok, start, runs);
      const status = classifyRange(start, end, runs);
      const wrapClass = status === 'added' ? 'diff-rendered-block-added' : '';
      parts.push(wrapClass ? `<div class="${wrapClass}">${html}</div>` : html);
    } else if (isBlockquote(tok)) {
      const status = classifyRange(start, end, runs);
      const html = marked.parser([tok]);
      const cls = statusClass(status);
      parts.push(cls ? `<div class="${cls}">${html}</div>` : html);
    } else if (tok.type !== 'space') {
      const status = classifyRange(start, end, runs);
      const html = marked.parser([tok]);
      const cls = statusClass(status);
      parts.push(cls ? `<div class="${cls}">${html}</div>` : html);
    }

    line += newlines;
  }

  return parts.join('');
}

function deletionLines(file: DiffFile): string[] {
  const out: string[] = [];
  for (const hunk of file.hunks) {
    for (const line of hunk.lines) {
      if (line.type === 'deletion') out.push(line.content);
    }
  }
  return out;
}

interface Strip { top: number; height: number; variant: 'added' | 'changed' }

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
  const deletions = useMemo(() => deletionLines(file), [file]);

  const html = useMemo(() => {
    if (content.status !== 'loaded') return null;
    return renderDiffMarked(content.data, runs);
  }, [content, runs]);

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
        '.diff-rendered-added, .diff-rendered-changed, .diff-rendered-block-added',
      );
      for (const el of Array.from(markedEls)) {
        const r = el.getBoundingClientRect();
        raw.push({
          top: r.top - containerRect.top + container.scrollTop,
          height: r.height,
          variant: el.classList.contains('diff-rendered-changed') ? 'changed' : 'added',
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
    return <div class="empty-state error-text">Failed to load: {content.error}</div>;
  }
  if (content.status !== 'loaded') {
    if (!showLoading) return null;
    return <div class="loading-spinner" />;
  }

  return (
    <div class="rendered-diff" ref={containerRef}>
      {deletions.length > 0 && (
        <details class="rendered-diff-deletions">
          <summary>−{deletions.length} removed line{deletions.length === 1 ? '' : 's'}</summary>
          <pre>{deletions.map(escapeHtml).join('\n')}</pre>
        </details>
      )}
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
