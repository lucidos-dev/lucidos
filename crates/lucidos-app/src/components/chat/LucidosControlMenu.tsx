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
  loadChatModels, lucidosModelChoices, LUCIDOS_TIER_VOCABULARY,
} from '../../store/actions/models';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';
import { pairLabelOf } from '../../store/modelSelection';
import { LUCIDOS_AGENT_LABEL } from '../../store/thread-events';
import { LucidosMarkIcon } from '../shared/icons';
import { Overlay } from '../shared/Overlay';
import { ModelSelectionPicker } from '../shared/ModelSelectionPicker';

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
  const open = useSignal(false);

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
  }

  useEffect(() => close, []);

  // Composing draft → THIS draft's override (?? account default); active thread →
  // this thread's pending pick ?? its last message ?? the account default.
  const modelValue = perDraft ? resolveModel(threadId) : resolveActiveThreadModel(threadId);
  const effortValue = perDraft
    ? resolveReasoningEffort(threadId)
    : resolveActiveThreadReasoningEffort(threadId);

  /** Where a pick lands. Never the account preference: that is a Settings
   *  default, and writing it here would leak the pick to every other draft and
   *  thread. */
  function applyPick(patch: ModelSelectionPatch) {
    const write = {
      ...(patch.model !== undefined ? { model: patch.model } : {}),
      ...(patch.reasoningEffort != null ? { reasoningEffort: patch.reasoningEffort } : {}),
    };
    if (perDraft) updateComposeSelection(threadId ?? null, write);
    else if (threadId) patchThreadModelOverride(threadId, write);
  }

  const selection = useModelSelection({
    models: lucidosModelChoices(modelValue),
    vocabulary: LUCIDOS_TIER_VOCABULARY,
    model: modelValue,
    effort: effortValue,
    onChange: applyPick,
  });

  function pick(encoded: string) {
    selection.pick(encoded);
    showToast(`Model: ${pairLabelOf(selection.rows, encoded)}`, 'success');
    close();
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
          else open.value = true;
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
      >
        <ModelSelectionPicker
          label={LUCIDOS_AGENT_LABEL}
          selection={selection}
          onPick={pick}
        />
      </Overlay>
    </div>
  );
}
