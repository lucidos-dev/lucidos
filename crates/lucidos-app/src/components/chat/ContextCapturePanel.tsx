import { useEffect, useMemo, useState } from 'preact/hooks';
import type { ContextSection, ContextCapture, Loadable } from '../../store/types';
import { toFailed } from '../../store/types';
import { formatTokens, contextPercent } from '../../utils/formatTokens';
import { groupSections, type RoleGroup, type InnerGroup } from './contextGrouping';
import { sectionTokenScale, headlineTokens, type TokenScale } from './sectionTokens';
import { fetchContextCapture, type ContextCapturePayload } from '../../api/threads';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { mergeContextCaptureSections, needsLazyFetch } from './loadStrippedSections';
import { missingBodyReason, type MissingBodyReason } from './missingBodyReason';
import { toolCountBySource } from './toolCountBySource';
import { contextViewer } from '../../store/store';
import { currentCaptureContext } from '../../store/actions/preferences';
import { openCaptureContextSetting } from '../../store/actions/menu';
import { DisclosureChevron } from '../shared/DisclosureChevron';
import { Disclosure } from '../shared/Disclosure';
import { LoadingFade } from '../shared/LoadingFade';
import { SkText, SkeletonProvider } from '../shared/Skeleton';


type CaptureFacts = { tokens: TokenScale; missingBody: MissingBodyReason };

function formatChars(n: number): string {
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return `${n}`;
}

function goToCaptureSetting() {
  contextViewer.value = null;
  openCaptureContextSetting();
}

function MissingBodyNote({ reason }: { reason: MissingBodyReason }) {
  switch (reason) {
    case 'section-never-recorded':
      return <>Lucidos does not record this section's body.</>;
    case 'capture-off-at-step':
      return <>Capture context per step was off when this step ran. It is on now, so later steps record section bodies.</>;
    case 'capture-off':
      return (
        <>
          Capture context per step is off, so section bodies are not recorded.{' '}
          <button type="button" class="accent-link" data-role="capture-context-link" onClick={goToCaptureSetting}>
            Turn it on in Settings
          </button>{' '}
          to see them on later steps.
        </>
      );
  }
}

function ContextSectionRow({ section, tokens, missingBody }: CaptureFacts & { section: ContextSection }) {
  const [open, setOpen] = useState(false);
  return (
    <div class="context-section" data-role="section-row">
      <button class="context-section-header" onClick={() => setOpen(!open)} aria-expanded={open}>
        <DisclosureChevron open={open} />
        <span class="context-section-name">{section.name}</span>
        <span class="context-section-chars">
          {formatChars(section.budget_delta_chars)} · ≈{formatTokens(tokens(section.budget_delta_chars))}
        </span>
      </button>
      <Disclosure open={open}>
        {section.content !== undefined ? (
          <pre class="context-section-content">{section.content}</pre>
        ) : (
          <div class="context-section-content empty">
            <MissingBodyNote reason={missingBody} />
          </div>
        )}
      </Disclosure>
    </div>
  );
}

function ContextInnerGroup({ group, ...facts }: CaptureFacts & { group: InnerGroup }) {
  const [open, setOpen] = useState(true);
  const totalChars = group.sections.reduce((a, s) => a + s.budget_delta_chars, 0);
  return (
    <div class="context-inner-group">
      <button class="context-inner-header" onClick={() => setOpen(!open)} aria-expanded={open}>
        <DisclosureChevron open={open} />
        <span class="context-inner-label">{group.name}</span>
        <span class="context-inner-chars">
          {formatChars(totalChars)} · ≈{formatTokens(facts.tokens(totalChars))}
        </span>
      </button>
      <Disclosure open={open}>
        {group.sections.map(s => <ContextSectionRow key={s.name} section={s} {...facts} />)}
      </Disclosure>
    </div>
  );
}

function ContextRoleGroup({ role, facts }: { role?: RoleGroup; facts?: CaptureFacts }) {
  const [open, setOpen] = useState(true);
  if (!role || !facts) {
    return (
      <div class="context-role" aria-hidden="true">
        <div class="context-role-header">
          <DisclosureChevron open={false} />
          <SkText class="context-role-label" w="7rem" />
          <SkText class="context-role-chars" w="9rem" />
        </div>
      </div>
    );
  }
  const totalChars = role.innerGroups
    .flatMap(ig => ig.sections)
    .reduce((a, s) => a + s.budget_delta_chars, 0);
  return (
    <div class="context-role">
      <button class="context-role-header" onClick={() => setOpen(!open)} aria-expanded={open}>
        <DisclosureChevron open={open} />
        <span class="context-role-label">{role.label}</span>
        <span class="context-role-chars">
          {formatChars(totalChars)}<span class="context-unit"> chars</span> · ≈{formatTokens(facts.tokens(totalChars))}<span class="context-unit"> tokens</span>
        </span>
      </button>
      <Disclosure open={open}>
        {role.innerGroups.map(ig => (
          ig.name
            ? <ContextInnerGroup key={ig.name} group={ig} {...facts} />
            : ig.sections.map(s => <ContextSectionRow key={s.name} section={s} {...facts} />)
        ))}
      </Disclosure>
    </div>
  );
}

const CONTEXT_SECTIONS_SKELETON = (
  <SkeletonProvider>
    <div class="step-detail-context-meta"><SkText w="16rem" /></div>
    <div class="context-sections">
      {[0, 1, 2].map((i) => <ContextRoleGroup key={i} />)}
    </div>
  </SkeletonProvider>
);

function ContextSectionsArea({ snap }: { snap: ContextCapture }) {
  const inlineLoadable: Loadable<ContextCapturePayload> = useMemo(() => ({
    status: 'loaded',
    data: { sections: snap.sections, tools: snap.tools },
  }), [snap]);
  const [loadable, setLoadable] = useState<Loadable<ContextCapturePayload>>(
    needsLazyFetch(snap) ? { status: 'loading' } : inlineLoadable,
  );

  useEffect(() => {
    if (!needsLazyFetch(snap)) {
      setLoadable(inlineLoadable);
      return;
    }
    const eventId = snap.event_id;
    if (!eventId) {
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
  return (
    <LoadingFade class="step-detail-fade" showSkeleton={showLoading} skeleton={CONTEXT_SECTIONS_SKELETON}>
      {loadable.status === 'loaded' && loadedSections(loadable.data)}
    </LoadingFade>
  );

  function loadedSections(data: ContextCapturePayload) {
    const hydrated = mergeContextCaptureSections(snap, data);
    const facts: CaptureFacts = {
      tokens: sectionTokenScale(hydrated),
      missingBody: missingBodyReason(hydrated, currentCaptureContext()),
    };
    const agent = hydrated.producer === 'claude_code' ? 'Claude Code' : hydrated.producer === 'codex' ? 'Codex' : null;
    return (
      <>
        <div class="step-detail-context-meta">
          <code>{hydrated.model || '(unknown model)'}</code>
          <span> · {agent ?? (hydrated.producer === 'auxiliary' ? 'Auxiliary' : 'Main LLM')}</span>
          <span> · {toolCountBySource(hydrated.tools, agent ?? 'Lucidos')}</span>
          {hydrated.legacy && <span class="context-legacy-badge" data-tooltip="Synthesized from legacy events">legacy capture</span>}
        </div>
        <div class="context-sections">
          {groupSections(hydrated.sections).map(role => (
            <ContextRoleGroup key={role.role} role={role} facts={facts} />
          ))}
        </div>
      </>
    );
  }
}

export function ContextCapturePanel({ snap }: { snap: ContextCapture }) {
  const used = headlineTokens(snap);
  const pct = contextPercent(used, snap.context_window);
  const cacheRead = snap.usage?.cache_read_tokens ?? 0;
  const cacheWrite = snap.usage?.cache_creation_tokens ?? 0;
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
