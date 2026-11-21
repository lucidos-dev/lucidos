import { signal } from '@preact/signals';
import { useEffect, useState } from 'preact/hooks';
import { useServerBackedField } from '../../hooks/useServerBackedField';
import { PREF_WORKSPACE_PROMPT_FOOTPRINT_SECTION_CEILING, PREF_WORKSPACE_PROMPT_FOOTPRINT_TOTAL_CEILING, PREF_WORKSPACE_PROMPT_FOOTPRINT_UNUSED_DAYS } from '@lucidos/preference-catalog';
import { fetchWorkspacePromptFootprint, type FootprintItem, type FootprintSection, type WorkspacePromptFootprint } from '../../api/client';
import { promptFootprintVersion } from '../../store/store';
import { setLoadingIfFresh, toFailed, type Loadable } from '../../store/types';
import { savePreference } from '../../store/actions/preferences';
import { sendSeededPrompt } from '../../store/actions/compose';
import { openAppById } from '../../store/actions/apps';
import { openWidgetInCanvas } from '../../store/actions/widget-actions';
import { openFilePreview } from '../../store/actions/artifacts';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { LoadableError } from '../shared/LoadableError';
import { SkBlock, SkText, SkeletonProvider } from '../shared/Skeleton';
import {
  PROMPT_FOOTPRINT_AUDIT_PROMPT,
  costlySections,
  findingsLine,
  itemsFor,
  kindLabel,
  looksUnused,
  meterFraction,
  usageLine,
  type ItemTab,
} from './promptFootprint';

const report = signal<Loadable<WorkspacePromptFootprint>>({ status: 'not-loaded' });

/** Keeps a loaded report on screen while it re-reads. */
async function loadReport(): Promise<void> {
  setLoadingIfFresh(report);
  try {
    report.value = { status: 'loaded', data: await fetchWorkspacePromptFootprint() };
  } catch (e) {
    report.value = toFailed(e);
  }
}

const chars = (n: number) => n.toLocaleString();

/** Where the item is edited: the app, the widget, or the knowhow file. An item
 *  with nowhere to go is plain text. */
function ItemName({ item }: { item: FootprintItem }) {
  let open: (() => void) | null = null;
  if (item.kind === 'app' || item.kind === 'app-knowhow') open = () => void openAppById(item.id, 'Prompt Footprint');
  else if (item.kind === 'reusable-widget') open = () => void openWidgetInCanvas(item.id, item.name);
  else if (item.kind === 'knowhow' && item.path) open = () => openFilePreview(item.path!);
  if (!open) return <span>{item.name}</span>;
  return <button type="button" class="accent-link" onClick={open}>{item.name}</button>;
}

function ItemRow({ item }: { item: FootprintItem }) {
  const used = usageLine(item);
  return (
    <div class="list-row prompt-footprint-item">
      <div class="list-row-info">
        <div class="title list-row-name"><ItemName item={item} /></div>
        <div class="list-row-details">
          <span>{chars(item.chars)} chars</span>
          <span>{kindLabel(item.kind)}</span>
          {item.clipped_chars > 0 && <span class="prompt-footprint-warn">{chars(item.clipped_chars)} clipped</span>}
          {used && <span class={looksUnused(item) ? 'prompt-footprint-warn' : undefined}>{used}</span>}
        </div>
      </div>
    </div>
  );
}

const TABS: { tab: ItemTab; label: string }[] = [
  { tab: 'largest', label: 'Largest' },
  { tab: 'clipped', label: 'Clipped' },
  { tab: 'unused', label: 'Unused' },
];

function Items({ data }: { data: WorkspacePromptFootprint }) {
  const [tab, setTab] = useState<ItemTab>('largest');
  const items = itemsFor(data, tab);
  return (
    <div class="settings-section">
      <div class="settings-section-title">Items</div>
      <div class="pill-bar prompt-footprint-tabs" role="tablist">
        {TABS.map(({ tab: t, label }) => {
          const count = t === 'largest' ? null : itemsFor(data, t).length;
          return (
            <button key={t} type="button" role="tab" aria-selected={tab === t} class={`pill-bar-btn${tab === t ? ' active' : ''}`} onClick={() => setTab(t)}>
              {count === null ? label : `${label} · ${count}`}
            </button>
          );
        })}
      </div>
      {items.length === 0
        ? <p class="settings-section-desc">{tab === 'unused' ? `Nothing unused for ${data.unused_days} days.` : 'Nothing clipped.'}</p>
        : <div class="list-rows prompt-footprint-items">{items.map((item) => <ItemRow key={`${item.kind}:${item.id}`} item={item} />)}</div>}
    </div>
  );
}

const LIMITS = [
  { pref: PREF_WORKSPACE_PROMPT_FOOTPRINT_SECTION_CEILING, label: 'Section ceiling (chars)', read: (d: WorkspacePromptFootprint) => d.section_ceiling },
  { pref: PREF_WORKSPACE_PROMPT_FOOTPRINT_TOTAL_CEILING, label: 'Total ceiling (chars)', read: (d: WorkspacePromptFootprint) => d.total_ceiling },
  { pref: PREF_WORKSPACE_PROMPT_FOOTPRINT_UNUSED_DAYS, label: 'Unused after (days)', read: (d: WorkspacePromptFootprint) => d.unused_days },
] as const;

/** A limit saves on blur or Enter, inside the catalog's range. The report
 *  re-reads on the `PreferencesChanged` that write announces. The row is keyed
 *  on the value in force, so a new value arrives as a fresh field. */
function LimitRow({ limit, value }: { limit: (typeof LIMITS)[number]; value: number }) {
  const [draft, setDraft] = useServerBackedField(String(value));
  function commit() {
    const n = Math.round(Number(draft));
    if (!Number.isFinite(n) || n < limit.pref.min || n > limit.pref.max || n === value) {
      setDraft(String(value));
      return;
    }
    void savePreference(limit.pref.key, String(n));
  }
  return (
    <div class="settings-row">
      <span class="settings-row-label">{limit.label}</span>
      <input
        type="number"
        class="text-input prompt-footprint-limit"
        min={limit.pref.min}
        max={limit.pref.max}
        value={draft}
        onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
        onBlur={commit}
        onKeyDown={(e) => { if (e.key === 'Enter') (e.target as HTMLInputElement).blur(); }}
      />
    </div>
  );
}

/** The headline, the bar and its key. With no `data`, inside a
 *  `SkeletonProvider`, it draws itself as the loading placeholder. */
function Overview({ data }: { data?: WorkspacePromptFootprint }) {
  const total = data ? chars(data.total_chars) : '';
  const system = data ? chars(data.system_prompt_chars) : '';
  return (
    <div class="settings-section">
      <div class="settings-overview-headline">
        <SkText class="settings-overview-headline-value" w="5rem">{total}</SkText>
        <SkText class="settings-overview-headline-sub" w="18rem">
          chars of workspace content on every turn, of {data && chars(data.total_ceiling)}
        </SkText>
      </div>
      <div class="settings-overview-bar" role="img" aria-label="The two prompt footprints">
        {data ? (
          <>
            <div class="prompt-footprint-bar-system" style={{ flexGrow: data.system_prompt_chars }} />
            <div class="prompt-footprint-bar-workspace" style={{ flexGrow: data.total_chars }} />
          </>
        ) : <SkBlock w="100%" h="100%" />}
      </div>
      <div class="settings-overview-key">
        <div class="settings-overview-key-row">
          <span class="settings-overview-swatch prompt-footprint-bar-workspace" aria-hidden="true" />
          <span class="settings-overview-key-label">Workspace prompt footprint<span class="settings-overview-key-note">yours to trim</span></span>
          <SkText class="settings-overview-key-value" w="5rem">{total} chars</SkText>
        </div>
        <div class="settings-overview-key-row">
          <span class="settings-overview-swatch prompt-footprint-bar-system" aria-hidden="true" />
          <span class="settings-overview-key-label">System prompt footprint<span class="settings-overview-key-note">fixed by the release</span></span>
          <SkText class="settings-overview-key-value" w="5rem">{system} chars</SkText>
        </div>
      </div>
    </div>
  );
}

/** One section against its ceiling. With no `section` it is a placeholder. */
function SectionRow({ section, ceiling = 0 }: { section?: FootprintSection; ceiling?: number }) {
  const over = section?.over_ceiling ? ' over' : '';
  return (
    <div class="prompt-footprint-section-row">
      <SkText class="prompt-footprint-section-name" w="8rem">{section?.title}</SkText>
      <div class="prompt-footprint-meter">
        {section && <div class={`prompt-footprint-meter-fill${over}`} style={{ width: `${meterFraction(section.chars, ceiling) * 100}%` }} />}
      </div>
      <SkText class={`prompt-footprint-section-chars${over}`} w="3rem">{section && chars(section.chars)}</SkText>
    </div>
  );
}

const SKELETON_SECTION_ROWS = 4;

function Report({ data }: { data: WorkspacePromptFootprint }) {
  const sections = costlySections(data);
  const empty = data.sections.length - sections.length;
  return (
    <>
      <Overview data={data} />
      <div class="settings-section">
        <div class="settings-overview-card">
          <div class="settings-overview-card-text">
            <div class="settings-overview-card-title">Trim it</div>
            <div class="settings-overview-card-figure">{findingsLine(data)}</div>
            <p class="settings-overview-card-note">The audit proposes the fixes. Nothing changes until you pick them.</p>
          </div>
          <button type="button" class="action-btn" onClick={() => void sendSeededPrompt(PROMPT_FOOTPRINT_AUDIT_PROMPT, 'start the audit')}>
            Run audit
          </button>
        </div>
      </div>
      <div class="settings-section">
        <div class="settings-section-title">Sections</div>
        <p class="settings-section-desc">Each against the {chars(data.section_ceiling)}-char section ceiling.</p>
        {sections.map((s) => <SectionRow key={s.id} section={s} ceiling={data.section_ceiling} />)}
        {empty > 0 && <p class="prompt-footprint-empty-note">{empty === 1 ? '1 more section is' : `${empty} more sections are`} empty in this workspace.</p>}
      </div>
      <Items data={data} />
      <div class="settings-section">
        <div class="settings-section-title">Limits</div>
        {LIMITS.map((limit) => <LimitRow key={`${limit.pref.key}:${limit.read(data)}`} limit={limit} value={limit.read(data)} />)}
      </div>
    </>
  );
}

/** Settings → System → Workspace Prompt Footprint (ADR 0413): the report the
 *  audit reads, the three preferences it is judged against, and a way into
 *  the audit. Fixes go through the audit only, never a button here. */
export function PromptFootprintPage() {
  const loadable = report.value;
  const skeleton = useDelayedLoading(loadable);
  useEffect(() => { void loadReport(); }, []);
  usePanelRefresh('prompt footprint', loadReport);
  useVersionedRefresh(promptFootprintVersion.value, false, () => void loadReport());

  switch (loadable.status) {
    case 'loaded':
      return <Report data={loadable.data} />;
    case 'failed':
      return (
        <div class="settings-section">
          <LoadableError noun="the prompt footprint" error={loadable.error} onRetry={() => void loadReport()} />
        </div>
      );
    default:
      return skeleton ? (
        <SkeletonProvider>
          <Overview />
          <div class="settings-section">
            {Array.from({ length: SKELETON_SECTION_ROWS }, (_, i) => <SectionRow key={i} />)}
          </div>
        </SkeletonProvider>
      ) : null;
  }
}
