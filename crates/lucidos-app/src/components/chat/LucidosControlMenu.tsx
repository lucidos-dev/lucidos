import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { currentModel, reasoningEffort, chatModels, showToast } from '../../store/store';
import { chatModelOptions, loadChatModels } from '../../store/actions/models';
import { availableReasoningLevels } from '../../store/models';
import { setCurrentModel, setReasoningEffort } from '../../store/actions/preferences';
import { LUCIDOS_AGENT_LABEL, displayModelName } from '../../store/thread-events';
import { LucidosMark } from '../shared/LucidosMark';
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
 *  Unlike CC's per-thread model/effort (carried on the thread via
 *  CodingAgentSettingsChanged events + pending overrides), the Lucidos Agent
 *  model and reasoning are the GLOBAL chat preferences (`chat_model` /
 *  `chat_reasoning_effort`, surfaced via the `currentModel` / `reasoningEffort`
 *  signals and also editable in Settings → Models). Selecting here drives those
 *  same preferences, so the chat send path (`sendMessage`, which reads the live
 *  signals) and Settings stay in lockstep — no pending-override dance needed. */
export function LucidosControlMenu() {
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
  const effortOptions = availableReasoningLevels(currentModel.value);
  const currentModelLabel = displayModelName(currentModel.value);
  const currentEffortLabel =
    effortOptions.find((l) => l.value === reasoningEffort.value)?.label ?? reasoningEffort.value;

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
    const cur = next === 'model' ? currentModel.value : reasoningEffort.value;
    view.value = next;
    highlightIndex.value = selectedOptionIndex(opts, cur);
  }

  function pickModel(value: string, label: string) {
    void setCurrentModel(value);
    showToast(`Model: ${label}`, 'success');
    close();
  }

  function pickEffort(value: string, label: string) {
    void setReasoningEffort(value);
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
        class="icon-btn commands-btn lucidos-commands-btn"
        data-tooltip={`${LUCIDOS_AGENT_LABEL} model`}
        aria-label={`${LUCIDOS_AGENT_LABEL} model`}
        onClick={() => {
          if (open.value) close();
          else openMenu();
        }}
      >
        <span class="lucidos-agent-glyph"><LucidosMark size="var(--icon-size-md)" /></span>
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
            renderOptions('Model', modelOptions, currentModel.value, pickModel)
          ) : (
            renderOptions('Reasoning', effortOptions, reasoningEffort.value, pickEffort)
          )}
      </Overlay>
    </div>
  );
}
