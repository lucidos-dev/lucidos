import { useEffect, useRef, useState } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import {
  copyLadderFromChat, deletePreference, getChatLadder, getCodingAgentLadder, getModelPrices, setLadderDefault, setPreference, sortLadder,
} from '../../api/client';
import { createTrigger } from '../../api/client/triggers';
import type {
  ApiResult, Band, CodingAgent, IntelligenceLadder, LadderSurface, ServedIntelligenceRung, LongPromptShare,
} from '../../api/types';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { loadTriggers, navigateToTrigger } from '../../store/actions/triggers';
import { CHAT_LADDER_PREF, LADDER_PREFS, LONG_PROMPT_SHARE_PREF } from '../../store/intelligenceLadderPrefs';
import { intelligenceLadderVersion, showConfirm, showToast, triggers } from '../../store/store';
import { errorDetail } from '../../utils/errorDetail';
import { Disclosure } from '../shared/Disclosure';
import { Explainer } from '../shared/Explainer';
import { SettingsPageLinkRow } from './SettingsPageLinkRow';
import { ChevronDownIcon, ChevronUpIcon, GripIcon, StarIcon } from '../shared/icons';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText, useSkeleton } from '../shared/Skeleton';
import { ToggleIconButton } from '../shared/ToggleIconButton';
import {
  draggedRung, keepPricesCurrentTrigger, KEEP_PRICES_CURRENT_SLUG, LADDER_SURFACES, ladderAnchor, ladderOverride,
  movedRung, percent, starredLadder, toggledRung, type PriceCheckCadence,
} from './intelligenceLadder';
import { useRowDrag } from '../../hooks/useRowDrag';

interface PageData {
  ladders: Record<LadderSurface, IntelligenceLadder>;
  pricesChecked: string;
}

async function loadPage(): Promise<PageData> {
  const [chat, claudeCode, codex, prices] = await Promise.all([
    getChatLadder(),
    getCodingAgentLadder('claude-code'),
    getCodingAgentLadder('codex'),
    getModelPrices(),
  ]);
  return { ladders: { chat, 'claude-code': claudeCode, codex }, pricesChecked: prices.checked };
}

async function write(what: string, request: Promise<ApiResult>): Promise<boolean> {
  try {
    const result = await request;
    if (!result.success) {
      showToast(result.error || `Failed to save ${what}`, 'error');
      return false;
    }
    return true;
  } catch (e) {
    showToast(`Failed to save ${what}: ${errorDetail(e)}`, 'error');
    return false;
  }
}

function ladderPref(surface: LadderSurface): string {
  return surface === 'chat' ? CHAT_LADDER_PREF : LADDER_PREFS[surface].ladder;
}

export function IntelligencePage() {
  const [refresh, setRefresh] = useState(0);
  const refreshSequence = useRef(0);
  const refetch = () => {
    const next = ++refreshSequence.current;
    setRefresh(next);
    return next;
  };
  const [stars, setStars] = useState<Partial<Record<LadderSurface, {
    ladder: IntelligenceLadder;
    afterRefresh?: number;
  }>>>({});
  const settles = useRef<Array<() => void>>([]);
  const { loadable, showLoading } = useLoadableFetch(loadPage, [refresh, intelligenceLadderVersion.value], {
    keepLoadedWhileRefetching: true,
    onSettled: () => {
      setStars((current) => {
        const remaining = { ...current };
        for (const { surface } of LADDER_SURFACES) {
          const after = remaining[surface]?.afterRefresh;
          if (after !== undefined && refresh >= after) delete remaining[surface];
        }
        return remaining;
      });
      const waiting = settles.current;
      settles.current = [];
      for (const settle of waiting) settle();
    },
  });
  usePanelRefresh('intelligence', () => new Promise<void>((resolve) => {
    settles.current.push(resolve);
    refetch();
  }));

  const [edits, setEdits] = useState<Partial<Record<LadderSurface, IntelligenceLadder>>>({});
  useEffect(() => setEdits({}), [loadable]);

  const star = async (ladder: IntelligenceLadder, rung: ServedIntelligenceRung, name: string) => {
    const { surface } = ladder;
    if (rung.is_default || stars[surface]) return;
    const next = starredLadder(ladder, rung);
    setStars((current) => ({ ...current, [surface]: { ladder: next } }));
    const saved = await write(`the ${name} default`, setLadderDefault(surface, rung));
    if (saved) {
      const afterRefresh = refetch();
      setStars((current) => ({ ...current, [surface]: { ladder: next, afterRefresh } }));
    } else {
      setStars((current) => ({ ...current, [surface]: undefined }));
      refetch();
    }
  };

  if (loadable.status === 'failed') {
    return (
      <div class="settings-section">
        <div class="settings-section-title">Intelligence</div>
        <LoadableError noun="intelligence ladders" error={loadable.error} />
      </div>
    );
  }
  return (
    <LoadingFade
      showSkeleton={showLoading}
      skeleton={(
        <>
          <PricesSection />
          <LadderSection surface={LADDER_SURFACES[0].surface} name={LADDER_SURFACES[0].name} customised={false}>
            <ListSkeletonOf count={10} containerClass="list-rows" row={() => <LadderRow />} />
          </LadderSection>
        </>
      )}
    >
      {loadable.status === 'loaded' && (
        <>
          <PricesSection checked={loadable.data.pricesChecked} />
          {LADDER_SURFACES.map(({ surface, name }) => {
            const served = loadable.data.ladders[surface];
            return (
              <LadderBlock
                key={surface}
                name={name}
                ladder={stars[surface]?.ladder ?? edits[surface] ?? served}
                pending={edits[surface] !== undefined || stars[surface] !== undefined}
                starring={stars[surface] !== undefined}
                onStar={(rung) => void star(edits[surface] ?? served, rung, name)}
                setLadder={(next) => setEdits((all) => ({ ...all, [surface]: next }))}
                header={surface === 'claude-code' && served.long_prompt_share
                  ? <LongPromptShareRow share={served.long_prompt_share} />
                  : null}
              />
            );
          })}
        </>
      )}
    </LoadingFade>
  );
}

export function IntelligenceLinkRow({ children }: { children: ComponentChildren }) {
  return (
    <SettingsPageLinkRow view="intelligence" button="Open Intelligence">
      {children}
    </SettingsPageLinkRow>
  );
}

function formatDay(day: string): string {
  return new Date(`${day}T00:00:00`).toLocaleDateString(undefined, {
    day: 'numeric', month: 'short', year: 'numeric',
  });
}

function PricesSection({ checked }: { checked?: string }) {
  const [cadence, setCadence] = useState<PriceCheckCadence>('weekly');
  const [creating, setCreating] = useState(false);
  const list = triggers.value;
  useEffect(() => {
    if (list.status === 'not-loaded') void loadTriggers();
  }, [list.status]);
  const keeper = list.status === 'loaded'
    ? list.data.find((t) => t.slug === KEEP_PRICES_CURRENT_SLUG)
    : undefined;

  const keepCurrent = async () => {
    setCreating(true);
    if (await write('the price trigger', createTrigger(keepPricesCurrentTrigger(cadence)))) await loadTriggers();
    setCreating(false);
  };

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="intelligence:prices">Prices</div>
      <div class="settings-row">
        <span class="settings-row-label">
          <SkText w="9rem">{checked && `Prices checked ${formatDay(checked)}`}</SkText>
          <Explainer title="Prices">
            <p>
              Every cost band reads the price table: the prices Lucidos ships, plus any your
              workspace keeps in <code>data/config/model-prices.json</code>.
            </p>
            <p>
              Keep prices current creates a trigger that checks them on the schedule you pick
              and tells you when one changes. Lucidos never creates it by itself.
            </p>
          </Explainer>
        </span>
        <div class="settings-row-options">
          {keeper ? (
            <>
              <span class="ladder-note">A trigger keeps them current</span>
              <button class="action-btn action-btn-secondary" onClick={() => void navigateToTrigger(keeper.id)}>
                Open trigger
              </button>
            </>
          ) : (
            <>
              <div class="segmented-control" role="group" aria-label="How often to check prices">
                {(['weekly', 'daily'] as const).map((c) => (
                  <button
                    key={c}
                    type="button"
                    class={`segmented-btn${cadence === c ? ' active' : ''}`}
                    aria-pressed={cadence === c}
                    onClick={() => setCadence(c)}
                  >
                    {c === 'weekly' ? 'Weekly' : 'Daily'}
                  </button>
                ))}
              </div>
              <button
                type="button"
                class="action-btn"
                disabled={!checked || list.status !== 'loaded' || creating}
                onClick={() => void keepCurrent()}
              >
                Keep prices current
              </button>
            </>
          )}
        </div>
      </div>
    </div>
  );
}

function LongPromptShareRow({ share }: { share: LongPromptShare }) {
  const [draft, setDraft] = useState(percent(share.effective));
  useEffect(() => setDraft(percent(share.effective)), [share.effective]);
  const line = `${share.line_tokens / 1000}k tokens`;
  const title = `Share of calls over ${line}`;
  const what = `the share of calls over ${line}`;

  return (
    <div class="settings-row ladder-share-row" data-search-anchor="intelligence:long-prompt-share">
      <span class="settings-row-label">
        {title}
        <Explainer title={title}>
          <p>
            Some models, such as Haiku 5.5, bill a whole call higher once its prompt passes
            {' '}{line}. Their cost bands move with this share.
          </p>
          <p>Unset, Lucidos measures it from the last 30 days of Claude Code calls.</p>
        </Explainer>
      </span>
      <div class="settings-row-options ladder-share-controls">
        <input
          type="range"
          min="0"
          max="100"
          step="1"
          aria-label={title}
          value={draft}
          onInput={(e) => setDraft(parseInt((e.currentTarget as HTMLInputElement).value))}
          onChange={(e) => {
            const value = (e.currentTarget as HTMLInputElement).value;
            void write(what, setPreference(LONG_PROMPT_SHARE_PREF, value));
          }}
        />
        <span class="ladder-band ladder-share-value">{draft}%</span>
        <button
          type="button"
          class="action-btn action-btn-secondary ladder-small-btn"
          disabled={share.set === null}
          onClick={() => void write(what, deletePreference(LONG_PROMPT_SHARE_PREF))}
        >
          {share.measured === null ? 'Reset' : `Use measured (${percent(share.measured)}%)`}
        </button>
      </div>
    </div>
  );
}

function isCompact(row: Element): boolean {
  return getComputedStyle(row).getPropertyValue('--ladder-compact').trim() === '1';
}

function ladderPurpose(surface: LadderSurface, name: string): string {
  return surface === 'chat'
    ? 'The models and efforts for Lucidos Agent threads, least to most intelligent. The model '
      + 'picker lists every model with a rung that is on. An agent that starts a thread or sets '
      + "a trigger's model can only pick a rung that is on."
    : `The models and efforts an agent may pick when it starts a ${name} thread, least to most `
      + 'intelligent. Agents can only pick rungs that are on. Your own picker still offers everything.';
}

function LadderSection({
  surface,
  name,
  customised,
  controls,
  header,
  footer,
  children,
}: {
  surface: LadderSurface;
  name: string;
  customised: boolean;
  controls?: ComponentChildren;
  header?: ComponentChildren;
  footer?: ComponentChildren;
  children: ComponentChildren;
}) {
  return (
    <div class="settings-section ladder-section">
      <div class="settings-section-title ladder-title" data-search-anchor={ladderAnchor(surface)}>
        <span>
          Intelligence ladder · {name}
          {customised && <span class="ladder-customised">Customised</span>}
          <Explainer title={`Intelligence ladder · ${name}`}>
            <p>{ladderPurpose(surface, name)}</p>
            <p>
              The star marks the default: what a thread runs on when nobody picked. Reset
              returns the order and the switches to the shipped ladder. Drag a rung by its grip
              to reorder it. On a phone, press and hold a rung, then drag it.
            </p>
            <p>
              Intelligence, cost and time are per task, as a multiple of the default. They are
              Lucidos's own rough estimate, weighted toward Artificial Analysis.
            </p>
          </Explainer>
        </span>
        {controls}
      </div>
      {header}
      <div class="ladder-grid ladder-head">
        <span />
        <span>Rung, least to most intelligent</span>
        <span>Intelligence</span>
        <span>Cost</span>
        <span>Time</span>
        <span class="ladder-control-head">Default</span>
        <span class="ladder-control-head">On</span>
      </div>
      {children}
      {footer}
    </div>
  );
}

export function offLadderDefault(ladder: IntelligenceLadder): string | null {
  const pair = ladder.default_rung;
  if (!pair || ladder.rungs.some((r) => r.on && r.is_default)) return null;
  const label = ladder.rungs.find((r) => r.model === pair.model)?.label ?? pair.model;
  return `Default: ${label} · ${pair.effort}, not on the ladder.`;
}

function LadderBlock({
  name,
  ladder,
  pending,
  starring,
  onStar,
  setLadder,
  header,
}: {
  name: string;
  ladder: IntelligenceLadder;
  pending: boolean;
  starring: boolean;
  onStar: (rung: ServedIntelligenceRung) => void;
  setLadder: (next: IntelligenceLadder) => void;
  header: ComponentChildren;
}) {
  const [showOff, setShowOff] = useState(false);
  const surface = ladder.surface;
  const offCount = ladder.rungs.filter((r) => !r.on).length;
  const ladderWhat = `the ${name} intelligence ladder`;
  const offDefault = offLadderDefault(ladder);

  const saveRungs = async (rungs: ServedIntelligenceRung[]) => {
    setLadder({ ...ladder, rungs });
    const value = ladderOverride(rungs, ladder.shipped_rungs);
    const request = value === null
      ? deletePreference(ladderPref(surface))
      : setPreference(ladderPref(surface), value);
    const saved = await write(ladderWhat, request);
    if (!saved) setLadder(ladder);
  };

  const { listRef, drag, rowProps, gripProps } = useRowDrag((from, to) => {
    if (starring) return;
    const next = draggedRung(ladder.rungs, from, to);
    if (next) void saveRungs(next);
  }, isCompact);

  const controls = (
    <span class="ladder-title-controls" inert={starring}>
      {offCount > 0 && (
        <label class="ladder-show-off">
          <input
            type="checkbox"
            checked={showOff}
            onChange={(e) => setShowOff((e.currentTarget as HTMLInputElement).checked)}
          />
          Show rungs not on the ladder ({offCount})
        </label>
      )}
      {surface !== 'chat' && <CopyFromChatButton agent={surface} name={name} />}
      <button
        type="button"
        class="action-btn action-btn-secondary ladder-small-btn"
        disabled={!ladder.customised}
        onClick={() => void write(ladderWhat, deletePreference(ladderPref(surface)))}
      >
        Reset
      </button>
    </span>
  );

  return (
    <LadderSection
      surface={surface}
      name={name}
      customised={ladder.customised}
      controls={controls}
      header={(
        <>
          {header}
          <Disclosure open={!ladder.climbs && !pending}>
            <SortNotice surface={surface} name={name} />
          </Disclosure>
        </>
      )}
      footer={offDefault && <div class="ladder-note">{offDefault}</div>}
    >
      <div class="list-rows" ref={listRef}>
        {ladder.rungs.map((rung, i) => (rung.on || showOff) && (
          <LadderRow
            key={`${rung.model}=${rung.effort}`}
            rung={rung}
            index={i}
            dragProps={rung.on && !starring ? rowProps : undefined}
            gripProps={rung.on && !starring ? gripProps : undefined}
            lift={drag?.from === i ? drag.dy : undefined}
            dropAt={drag && drag.to === i && drag.to !== drag.from ? (drag.to > drag.from ? 'after' : 'before') : undefined}
            onStar={starring ? undefined : () => onStar(rung)}
            onMove={(step) => {
              if (starring) return null;
              const next = movedRung(ladder.rungs, i, step);
              return next && (() => void saveRungs(next));
            }}
            onToggle={starring || rung.is_default ? undefined : () => void saveRungs(toggledRung(ladder.rungs, i))}
          />
        ))}
      </div>
    </LadderSection>
  );
}

function SortNotice({ surface, name }: { surface: LadderSurface; name: string }) {
  const [sorting, setSorting] = useState(false);
  const sort = async () => {
    setSorting(true);
    try {
      await sortLadder(surface);
    } catch (e) {
      showToast(`Failed to sort the ${name} ladder by intelligence: ${errorDetail(e)}`, 'error');
    } finally {
      setSorting(false);
    }
  };
  return (
    <div class="ladder-notice">
      <span>This order does not climb: a more intelligent rung sits above a less intelligent one.</span>
      <button
        type="button"
        class="action-btn ladder-small-btn"
        disabled={sorting}
        onClick={() => void sort()}
      >
        Sort by intelligence
      </button>
    </div>
  );
}

function CopyFromChatButton({ agent, name }: { agent: CodingAgent; name: string }) {
  const [copying, setCopying] = useState(false);
  const copy = async () => {
    const ok = await showConfirm(
      `Replace the ${name} ladder with the Lucidos Agent's rungs that ${name} runs?\n\n`
        + `They keep their on and off state and take ${name}'s order. Reset brings back the shipped ladder.`,
      'Copy',
      { variant: 'default' },
    );
    if (!ok) return;
    setCopying(true);
    try {
      const { rungs } = await copyLadderFromChat(agent);
      showToast(`The ${name} ladder now has ${rungs.length} rungs from the Lucidos Agent's`, 'success');
    } catch (e) {
      showToast(`Failed to copy the Lucidos Agent's ladder to ${name}: ${errorDetail(e)}`, 'error');
    } finally {
      setCopying(false);
    }
  };
  return (
    <button
      type="button"
      class="action-btn action-btn-secondary ladder-small-btn"
      disabled={copying}
      onClick={() => void copy()}
    >
      Copy from Lucidos Agent
    </button>
  );
}

function LadderRow({
  rung,
  index,
  dragProps,
  gripProps,
  lift,
  dropAt,
  onStar,
  onMove,
  onToggle,
}: {
  rung?: ServedIntelligenceRung;
  index?: number;
  dragProps?: ReturnType<typeof useRowDrag>['rowProps'];
  gripProps?: ReturnType<typeof useRowDrag>['gripProps'];
  lift?: number;
  dropAt?: 'before' | 'after';
  onStar?: () => void;
  onMove?: (step: -1 | 1) => (() => void) | null;
  onToggle?: () => void;
}) {
  const sk = useSkeleton();
  const name = rung ? `${rung.label} at ${rung.effort}` : '';
  const up = onMove?.(-1) ?? null;
  const down = onMove?.(1) ?? null;
  return (
    <div
      class={`list-row ladder-row${rung && !rung.on ? ' ladder-row-off' : ''}${lift !== undefined ? ' ladder-row-lifted' : ''}${dropAt ? ` ladder-row-drop-${dropAt}` : ''}`}
      data-row-index={index}
      style={lift !== undefined ? { '--ladder-drag-dy': `${lift}px` } : undefined}
      {...dragProps}
    >
      <div class="ladder-grid">
        <span class={`ladder-grip${gripProps ? ' ladder-grip-live' : ''}`} {...gripProps}>
          {(sk || gripProps) && <SkBlock w="0.75rem" h="1rem"><GripIcon /></SkBlock>}
        </span>
        <SkText class="title" w="9rem">{rung && `${rung.label} · ${rung.effort}`}</SkText>
        <BandCell label="Intelligence" band={rung?.intelligence} loaded={!!rung} />
        <BandCell label="Cost" band={rung?.cost} loaded={!!rung} />
        <BandCell label="Time" band={rung?.time} loaded={!!rung} />
        <span class="ladder-actions">
          <SkBlock w="1.5rem" h="1.5rem" round>
            <ToggleIconButton
              tone="glyph"
              pressed={!!rung?.is_default}
              aria-label={rung?.is_default ? `${name} is the default` : `Make ${name} the default`}
              disabled={!onStar}
              onClick={onStar}
            >
              <StarIcon filled={!!rung?.is_default} />
            </ToggleIconButton>
          </SkBlock>
          <span class="ladder-order">
            {!sk && (
              <>
                <button type="button" class="icon-btn ladder-move" aria-label={`Move ${name} toward less intelligent`} disabled={!up} onClick={() => up?.()}>
                  <ChevronUpIcon size="1rem" />
                </button>
                <button type="button" class="icon-btn ladder-move" aria-label={`Move ${name} toward more intelligent`} disabled={!down} onClick={() => down?.()}>
                  <ChevronDownIcon size="1rem" />
                </button>
              </>
            )}
          </span>
          <SkBlock w="2.25rem" h="1.25rem" round>
            <label class="toggle-switch">
              <input type="checkbox" aria-label={`${name} on the ladder`} checked={!!rung?.on} disabled={!onToggle} onChange={onToggle} />
              <span class="toggle-slider" />
            </label>
          </SkBlock>
        </span>
      </div>
    </div>
  );
}

function BandCell({ label, band, loaded }: { label: string; band?: Band | null; loaded: boolean }) {
  return (
    <span class="ladder-cell">
      <span class="ladder-cell-label">{label}</span>
      <SkText class="ladder-band" w="2.5rem">{loaded && (band?.label ?? 'not estimated')}</SkText>
    </span>
  );
}
