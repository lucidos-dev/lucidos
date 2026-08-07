import { useEffect, useMemo, useState } from 'preact/hooks';
import type { ContextSection, ContextCapture, Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { formatTokens, contextPercent } from '../../utils/formatTokens';
import { groupSections, type RoleGroup, type InnerGroup } from './contextGrouping';
import { sectionTokenScale, headlineTokens, type TokenScale } from './sectionTokens';
import { fetchContextCapture, type ContextCapturePayload } from '../../api/threads';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { mergeContextCaptureSections, needsLazyFetch } from './loadStrippedSections';

// The body of the context viewer: what the model was actually sent for one LLM
// call. Lives in its own module because it is reached from the step row's
// context counter (`ContextViewerModal`) rather than from the step detail, and
// a panel this size sitting inside another modal's file is how the counter's
// view and the step's view would drift.

function formatChars(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return `${n}`;
}

/** `tokens` is the scale from `sectionTokenScale`: a share of the capture's
 *  headline total, threaded down from `ContextSectionsArea` so every row in
 *  the tree divides the same measured (or same estimated) pie. */
function ContextSectionRow({ section, tokens }: { section: ContextSection; tokens: TokenScale }) {
  const [open, setOpen] = useState(false);
  return (
    <div class="context-section" data-role="section-row">
      <button class="context-header context-section-header" onClick={() => setOpen(!open)}>
        <span>{open ? '▼' : '▶'}</span>
        <span class="context-section-name">{section.name}</span>
        <span class="context-section-chars">
          {formatChars(section.char_count)} chars · ≈{formatTokens(tokens(section.char_count))} tokens
        </span>
      </button>
      {open && section.content !== undefined && (
        <pre class="context-section-content">{section.content}</pre>
      )}
      {open && section.content === undefined && (
        <div class="context-section-content empty">
          Body not captured for this section.
        </div>
      )}
    </div>
  );
}

function ContextInnerGroup({ group, tokens }: { group: InnerGroup; tokens: TokenScale }) {
  const [open, setOpen] = useState(true);
  const totalChars = group.sections.reduce((a, s) => a + s.char_count, 0);
  return (
    <div class="context-inner-group">
      <button class="context-inner-header" onClick={() => setOpen(!open)}>
        <span>{open ? '▾' : '▸'}</span>
        <span class="context-inner-label">{group.name}</span>
        <span class="context-inner-chars">
          {formatChars(totalChars)} · ≈{formatTokens(tokens(totalChars))}
        </span>
      </button>
      {open && group.sections.map(s => <ContextSectionRow key={s.name} section={s} tokens={tokens} />)}
    </div>
  );
}

function ContextRoleGroup({ role, tokens }: { role: RoleGroup; tokens: TokenScale }) {
  const [open, setOpen] = useState(true);
  const totalChars = role.innerGroups
    .flatMap(ig => ig.sections)
    .reduce((a, s) => a + s.char_count, 0);
  return (
    <div class="context-role">
      <button class="context-role-header" onClick={() => setOpen(!open)}>
        <span>{open ? '▼' : '▶'}</span>
        <span class="context-role-label">{role.label}</span>
        <span class="context-role-chars">
          {formatChars(totalChars)} chars · ≈{formatTokens(tokens(totalChars))} tokens
        </span>
      </button>
      {open && role.innerGroups.map(ig => (
        ig.name
          ? <ContextInnerGroup key={ig.name} group={ig} tokens={tokens} />
          : ig.sections.map(s => <ContextSectionRow key={s.name} section={s} tokens={tokens} />)
      ))}
    </div>
  );
}

/** Sections + tools area. Lazy-fetches when the server stripped them on the
 *  snapshot endpoint (see `api/threads.rs :: strip_context_capture_sections`).
 *  Renders loading/error/loaded states from a `Loadable<ContextCapturePayload>`
 *  per `.claude/rules/frontend.md`'s "Async Data Loading" rule. The
 *  surrounding inline-chip fields (tokens, model, usage) render synchronously
 *  from `snap` either way; only the sections + tools detail block needs the
 *  async hydration. */
function ContextSectionsArea({ snap }: { snap: ContextCapture }) {
  // Synchronous inline-rendered captures (live SSE, legacy synth, or any
  // path where the server didn't strip) resolve to a `loaded` Loadable with
  // the existing fields, so the render path below is uniform.
  const inlineLoadable: Loadable<ContextCapturePayload> = useMemo(() => ({
    status: 'loaded',
    data: { sections: snap.sections, tools: snap.tools },
  }), [snap]);
  const [loadable, setLoadable] = useState<Loadable<ContextCapturePayload>>(
    needsLazyFetch(snap) ? { status: 'loading' } : inlineLoadable,
  );

  // Deps include `inlineLoadable` (a useMemo keyed on `snap`, already in deps):
  // its identity is stable across the in-flight fetch, and the `cancelled` flag
  // guards the async write so a late resolve can't clobber a newer snapshot.
  // Keep both invariants if you ever refactor this dependency list.
  useEffect(() => {
    if (!needsLazyFetch(snap)) {
      setLoadable(inlineLoadable);
      return;
    }
    const eventId = snap.event_id;
    if (!eventId) {
      // Stripped marker without an event id is an upstream contract break.
      // Toast is wrong here: the inline `failed` Loadable below already
      // surfaces the issue in the open viewer where the user is looking,
      // and the next open of the same step would re-trigger the same fetch
      // attempt (self-recovering once the upstream fix lands).
      console.warn('[ContextCapturePanel] ContextCapture is sections_stripped but has no event_id; cannot lazy-fetch.');
      setLoadable(toFailed<ContextCapturePayload>(new Error('missing event id')));
      return;
    }
    let cancelled = false;
    setLoadable({ status: 'loading' });
    fetchContextCapture(eventId)
      .then(payload => {
        if (cancelled) return;
        setLoadable({ status: 'loaded', data: payload });
      })
      .catch((err: unknown) => {
        if (cancelled) return;
        setLoadable(toFailed<ContextCapturePayload>(err));
      });
    return () => { cancelled = true; };
  }, [snap, inlineLoadable]);

  const showLoading = useDelayedLoading(loadable);
  if (loadable.status === 'failed') {
    return <div class="context-sections-error" data-role="context-sections-error">Failed to load sections: {loadable.error}</div>;
  }
  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return <div class="context-sections-loading" data-role="context-sections-loading">Loading sections…</div>;
  }
  const hydrated = mergeContextCaptureSections(snap, loadable.data);
  // Built from the HYDRATED capture, never from `snap`: a stripped snapshot
  // carries no sections, so a scale derived before the lazy fetch would divide
  // by zero and flatten every row to 0 tokens.
  const tokens = sectionTokenScale(hydrated);
  return (
    <>
      <div class="step-detail-context-meta">
        <code>{hydrated.model || '(unknown model)'}</code>
        <span> · {hydrated.producer === 'claude_code' ? 'Claude Code' : hydrated.producer === 'codex' ? 'Codex' : 'Main LLM'}</span>
        <span> · {hydrated.tools.length} tools</span>
        {hydrated.legacy && <span class="context-legacy-badge" data-tooltip="Synthesized from legacy events">legacy capture</span>}
      </div>
      <div class="context-sections">
        {groupSections(hydrated.sections).map(role => (
          <ContextRoleGroup key={role.role} role={role} tokens={tokens} />
        ))}
      </div>
    </>
  );
}

/** Budget bar + section list + (when usage is present) cache breakdown. */
export function ContextCapturePanel({ snap }: { snap: ContextCapture }) {
  // Same function `sectionTokenScale` divides up, so the tree below cannot
  // disagree with this bar. Do not inline the expression here.
  const used = headlineTokens(snap);
  const pct = contextPercent(used, snap.context_window);
  const cacheRead = snap.usage?.cache_read_tokens ?? 0;
  const cacheWrite = snap.usage?.cache_creation_tokens ?? 0;
  // Real input minus what was cached: what the user paid full price for.
  const cacheMiss = snap.usage
    ? Math.max(0, snap.usage.input_tokens - cacheRead - cacheWrite)
    : 0;
  return (
    <div class="step-detail-context">
      <div class="context-budget" data-role="budget-bar">
        <div class="context-budget-row">
          <span>
            <strong>{formatTokens(used)}</strong> / {formatTokens(snap.context_window)} tokens
            {' '}({pct}%){snap.usage ? '' : ' (est.)'}
          </span>
          {snap.trimmed && <span class="context-budget-trimmed">trimmed</span>}
        </div>
        <div class="progress-bar">
          <div class="progress-bar-fill" style={`width: ${pct}%`} />
        </div>
      </div>
      <ContextSectionsArea snap={snap} />
      {snap.usage && (
        <div class="context-usage" data-role="usage-row">
          <span>input <strong>{formatTokens(snap.usage.input_tokens)}</strong></span>
          <span>output <strong>{formatTokens(snap.usage.output_tokens)}</strong></span>
          <span>cache: read <strong>{formatTokens(cacheRead)}</strong> · write <strong>{formatTokens(cacheWrite)}</strong> · miss <strong>{formatTokens(cacheMiss)}</strong></span>
        </div>
      )}
    </div>
  );
}
