import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { chatModels, showToast } from '../../store/store';
import { resolveModel, resolveReasoningEffort } from '../../store/composeSelections';
import {
  resolveActiveThreadModel,
  resolveActiveThreadReasoningEffort,
  patchThreadModelOverride,
} from '../../store/threadModelSelections';
import { updateComposeSelection } from '../../store/actions/compose';
import {
  chatModelOptions, clampEffortFor, loadChatModels, reasoningLevelsFor,
} from '../../store/actions/models';
import { LUCIDOS_AGENT_LABEL, displayModelName } from '../../store/thread-events';
import { LucidosMarkIcon } from '../shared/icons';
import { Overlay } from '../shared/Overlay';
import { focusIfNeeded } from './promptFocus';

type View = 'root' | 'model' | 'effort';

/** Move a highlight index by one step within `[0, count)`, wrapping at both
 *  ends (down past the last row lands on the first, up past the first lands on
 *  the last). Returns 0 for an empty list. */
export function wrapHighlight(current: number, count: number, delta: 1 | -1): number {
  if (count <= 0) return 0;
  return (current + delta + count) % count;
}

/** Index of `currentValue` in `options`, or 0 when it isn't present — so
 *  drilling into a sub-menu always pre-highlights a valid, sensible row. */
export function selectedOptionIndex(options: Array<{ value: string }>, currentValue: string): number {
  const idx = options.findIndex((o) => o.value === currentValue);
  return idx >= 0 ? idx : 0;
}

/** The Lucidos Agent's model + reasoning picker — the chat-agent sibling of
 *  {@link CodingAgentControlMenu}. Mounted in the prompt-bar actions row whenever the
 *  compose destination is the Lucidos Agent, in the same slot the coding-agent
 *  control button occupies for Claude Code / Codex.
 *
 *  The Lucidos Agent's model + reasoning are remembered PER THREAD (like CC's,
 *  but derived from the model/effort the backend stamps on each MessageReceived
 *  rather than CodingAgentSettingsChanged events). On an ACTIVE thread the picker
 *  reads `resolveActiveThreadModel`/`resolveActiveThreadReasoningEffort` (this
 *  thread's pending pick ?? its last message ?? the account default) and a pick
 *  writes THIS thread's pending override (`threadModelSelections`) — it does NOT
 *  touch the account preference, so it never leaks to other threads. The account
 *  default (`chat_model` / `chat_reasoning_effort`) is set in Settings → Models
 *  and is the fallback for a brand-new thread.
 *
 *  In the COMPOSE view, the pick is per-draft instead: it reads/writes THIS
 *  draft's override in `composeSelections` (or the PENDING slot before a draft
 *  exists — `threadId` undefined on the fresh compose view) and never touches
 *  the account preference, so changing the model on one draft can't change
 *  another draft or the saved default (draft-only). The draft's pick is carried
 *  into the send by `sendCompose`. `composeContext` is true for both a focused
 *  composing draft and the fresh no-draft compose view. */
export function LucidosControlMenu({ threadId, composeContext }: { threadId?: string; composeContext?: boolean }) {
  const perDraft = !!composeContext;
  const menuRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLDivElement>(null);
  const open = useSignal(false);
  const view = useSignal<View>('root');
  const highlightIndex = useSignal(0);

  // Idempotent render-path kick-off so the picker shows the live DB-backed
  // registry; `chatModelOptions()` falls back to the static MODELS list until
  // it lands. `loadChatModels` single-flights via setLoadingIfFresh.
  if (chatModels.value.status === 'not-loaded') void loadChatModels();

  function close() {
    // Blur before DOM removal so any focusout-driven header restore fires while
    // the element is still connected (same convention as CodingAgentControlMenu).
    const active = document.activeElement as HTMLElement | null;
    if (active && menuRef.current?.contains(active)) active.blur();
    open.value = false;
    view.value = 'root';
    highlightIndex.value = 0;
  }

  useEffect(() => close, []);

  const modelOptions = chatModelOptions();
  // Composing draft → THIS draft's override (?? account default); active thread →
  // this thread's pending pick ?? its last message ?? the account default.
  const modelValue = perDraft ? resolveModel(threadId) : resolveActiveThreadModel(threadId);
  const effortValue = perDraft
    ? resolveReasoningEffort(threadId)
    : resolveActiveThreadReasoningEffort(threadId);
  const effortOptions = reasoningLevelsFor(modelValue);
  const currentModelLabel = displayModelName(modelValue);
  const currentEffortLabel =
    effortOptions.find((l) => l.value === effortValue)?.label ?? effortValue;

  const rootItems = [
    { key: 'model' as const, label: 'Model', current: currentModelLabel },
    { key: 'effort' as const, label: 'Reasoning', current: currentEffortLabel },
  ];

  const itemCount =
    view.value === 'root'
      ? rootItems.length
      : view.value === 'model'
        ? modelOptions.length
        : effortOptions.length;

  function openMenu() {
    open.value = true;
    view.value = 'root';
    highlightIndex.value = 0;
  }

  /** Drill into a sub-menu, pre-highlighting the currently-selected option. */
  function enterView(next: 'model' | 'effort') {
    const opts = next === 'model' ? modelOptions : effortOptions;
    const cur = next === 'model' ? modelValue : effortValue;
    view.value = next;
    highlightIndex.value = selectedOptionIndex(opts, cur);
  }

  function pickModel(value: string, label: string) {
    // Keep the effort valid for the newly picked model, whichever store we
    // write. The new model may support fewer tiers than the old one, and an
    // effort left behind would be clamped by the engine anyway, silently.
    const clamped = clampEffortFor(effortValue, value);
    const patch = clamped !== effortValue ? { model: value, reasoningEffort: clamped } : { model: value };
    if (perDraft) {
      // Per-draft override (persisted via the debounced compose PUT), or the
      // PENDING slot before a draft exists — never the account preference.
      updateComposeSelection(threadId ?? null, patch);
    } else if (threadId) {
      // Active thread: THIS thread's pending pick only — never the account
      // preference (the account default lives in Settings → Models).
      patchThreadModelOverride(threadId, patch);
    }
    showToast(`Model: ${label}`, 'success');
    close();
  }

  function pickEffort(value: string, label: string) {
    if (perDraft) {
      updateComposeSelection(threadId ?? null, { reasoningEffort: value });
    } else if (threadId) {
      patchThreadModelOverride(threadId, { reasoningEffort: value });
    }
    showToast(`Reasoning: ${label}`, 'success');
    close();
  }

  function activateHighlighted() {
    if (view.value === 'root') {
      enterView(rootItems[highlightIndex.value].key);
    } else if (view.value === 'model') {
      const o = modelOptions[highlightIndex.value];
      if (o) pickModel(o.value, o.label);
    } else {
      const o = effortOptions[highlightIndex.value];
      if (o) pickEffort(o.value, o.label);
    }
  }

  function handleKeyDown(e: KeyboardEvent) {
    if (e.key === 'Escape') {
      e.preventDefault();
      if (view.value !== 'root') {
        view.value = 'root';
        highlightIndex.value = 0;
      } else {
        close();
      }
      return;
    }
    if (e.key === 'Enter') {
      e.preventDefault();
      activateHighlighted();
      return;
    }
    if (e.key === 'ArrowDown') {
      e.preventDefault();
      highlightIndex.value = wrapHighlight(highlightIndex.value, itemCount, 1);
      return;
    }
    if (e.key === 'ArrowUp') {
      e.preventDefault();
      highlightIndex.value = wrapHighlight(highlightIndex.value, itemCount, -1);
    }
  }

  // Focus the list on open / view change so keyboard nav works without a click.
  useEffect(() => {
    if (open.value) requestAnimationFrame(() => focusIfNeeded(listRef.current));
  }, [open.value, view.value]);

  // Keep the highlighted row visible — the model list can overflow the dropdown.
  useEffect(() => {
    if (!open.value) return;
    listRef.current?.querySelector('.control-item-active')?.scrollIntoView({ block: 'nearest' });
  }, [highlightIndex.value]);

  function renderOptions(
    label: string,
    options: Array<{ value: string; label: string }>,
    currentValue: string,
    onPick: (value: string, label: string) => void,
  ) {
    return (
      <div class="control-list" tabIndex={0} ref={listRef}>
        <div class="control-section-label">{label}</div>
        {options.map((opt, i) => {
          const isCurrent = opt.value === currentValue;
          return (
            <button
              key={opt.value}
              class={`control-item control-option${i === highlightIndex.value ? ' control-item-active' : ''}${isCurrent ? ' control-option-current' : ''}`}
              onClick={() => onPick(opt.value, opt.label)}
              onMouseEnter={() => {
                highlightIndex.value = i;
              }}
            >
              <span class="control-option-label">
                {isCurrent && <span class="control-checkmark">&#10003;</span>}
                {opt.label}
              </span>
            </button>
          );
        })}
      </div>
    );
  }

  return (
    <div class="control-menu" data-row-item ref={menuRef}>
      <button
        // `lucidos-commands-btn` is the marker that distinguishes the Lucidos
        // Agent model picker from the coding-agent control button (both share
        // the `commands-btn` base style). Surfaces that need only the
        // coding-agent control — e.g. the `.commands-btn:not(.lucidos-commands-btn)`
        // e2e selector — rely on it, so keep it on this button.
        class="icon-btn header-icon commands-btn lucidos-commands-btn"
        data-tooltip={`${LUCIDOS_AGENT_LABEL} model`}
        aria-label={`${LUCIDOS_AGENT_LABEL} model`}
        onClick={() => {
          if (open.value) close();
          else openMenu();
        }}
      >
        {/* The flat mark, not `<LucidosMark/>`: this button is one of the
            prompt bar's gray icons, so the glyph paints in `currentColor` and
            carries no gradient tile. `.icon-btn.header-icon svg` sizes it. */}
        <LucidosMarkIcon />
      </button>
      <Overlay
        open={open.value}
        onClose={close}
        anchor={menuRef.current}
        backdrop={false}
        panelClass="control-dropdown control-dropdown-auto"
        panelProps={{ onKeyDown: handleKeyDown }}
      >
          {view.value === 'root' ? (
            <div class="control-list" tabIndex={0} ref={listRef}>
              <div class="control-section-label">{LUCIDOS_AGENT_LABEL}</div>
              {rootItems.map((item, i) => (
                <button
                  key={item.key}
                  class={`control-item${i === highlightIndex.value ? ' control-item-active' : ''}`}
                  onClick={() => enterView(item.key)}
                  onMouseEnter={() => {
                    highlightIndex.value = i;
                  }}
                >
                  {item.label}
                  <span class="control-current-value"> · {item.current}</span>
                </button>
              ))}
            </div>
          ) : view.value === 'model' ? (
            renderOptions('Model', modelOptions, modelValue, pickModel)
          ) : (
            renderOptions('Reasoning', effortOptions, effortValue, pickEffort)
          )}
      </Overlay>
    </div>
  );
}
