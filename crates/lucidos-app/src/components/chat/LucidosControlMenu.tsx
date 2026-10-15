import { useSignal } from '@preact/signals';
import { useEffect, useRef } from 'preact/hooks';
import { chatModels, showToast } from '../../store/store';
import {
  resolveModel, resolveProvider, resolveReasoningEffort,
} from '../../store/composeSelections';
import {
  resolveActiveThreadModel,
  resolveActiveThreadProvider,
  resolveActiveThreadReasoningEffort,
  patchThreadModelOverride,
} from '../../store/threadModelSelections';
import { updateComposeSelection } from '../../store/actions/compose';
import {
  defaultTierLabel, loadChatModels, lucidosModelChoices, LUCIDOS_TIER_VOCABULARY,
  PROVIDER_DEFAULT_EFFORT_LABEL, rememberProvider,
} from '../../store/actions/models';
import { accountChatEffort } from '../../store/actions/preferences';
import { useModelSelection, type ModelSelectionPatch } from '../../hooks/useModelSelection';
import { pairLabelOf } from '../../store/modelSelection';
import { LUCIDOS_AGENT_LABEL } from '../../store/thread-events';
import { Overlay } from '../shared/Overlay';
import { anchoredPanelStyle, useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { viewportIsMobile } from '../../utils/viewport';
import { ModelSelectionPicker, controlMenuParts } from './controlMenuParts';
import { takeControlMenuPress } from './controlMenuRequests';
import { ControlMenuTrigger, LUCIDOS_MENU_LABEL, LUCIDOS_TRIGGER_CLASS, LucidosMenuIcon } from './ControlMenuTrigger';
import { usePendingOpen } from '../../hooks/usePendingOpen';
import { keyboardHolder, returnKeyboard } from '../shared/keyboardHandoff';

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
 *  default (`chat_model` / `chat_reasoning_efforts`) is set in Settings → Models
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
  const panelRef = useRef<HTMLDivElement>(null);
  const open = useSignal(false);
  const pendingOpen = usePendingOpen();
  // The prompt, when it held the keyboard as the menu opened. The filter box
  // takes the keyboard from it, and closing hands it back.
  const keyboardHolderRef = useRef<HTMLElement | null>(null);
  // Placed like CodingAgentControlMenu, whose `.control-dropdown` box it shares.
  const pos = useAnchoredPosition(
    open.value ? menuRef.current : null,
    panelRef,
    '.thread-pane',
    viewportIsMobile.value ? 'container-center' : 'start',
  );

  // Idempotent render-path kick-off so the picker shows the live DB-backed
  // registry; `chatModelOptions()` falls back to the static MODELS list until
  // it lands. `loadChatModels` single-flights via setLoadingIfFresh.
  if (chatModels.value.status === 'not-loaded') void loadChatModels();

  function close() {
    returnKeyboard(keyboardHolderRef.current, panelRef.current);
    keyboardHolderRef.current = null;
    // Blur before DOM removal so any focusout-driven header restore fires while
    // the element is still connected (same convention as CodingAgentControlMenu).
    const active = document.activeElement as HTMLElement | null;
    if (active && (menuRef.current?.contains(active) || panelRef.current?.contains(active))) active.blur();
    open.value = false;
  }

  useEffect(() => close, []);

  function openPicker(trigger: HTMLElement) {
    const holder = keyboardHolder();
    pendingOpen.open(controlMenuParts, () => {
      keyboardHolderRef.current = holder;
      open.value = true;
    }, trigger);
  }

  // A press that landed on the placeholder before this code arrived.
  useEffect(() => {
    if (menuRef.current && takeControlMenuPress('lucidos')) openPicker(menuRef.current);
  }, []);

  // Composing draft → THIS draft's override (?? account default); active thread →
  // this thread's pending pick ?? its last message ?? the account default.
  const modelValue = perDraft ? resolveModel(threadId) : resolveActiveThreadModel(threadId);
  const effortValue = perDraft
    ? resolveReasoningEffort(threadId)
    : resolveActiveThreadReasoningEffort(threadId, modelValue);
  // The tier another model would run at: the account's for a draft, and the
  // thread's own memory for that model on an active thread.
  const effortFor = (model: string) => (perDraft
    ? accountChatEffort(model)
    : resolveActiveThreadReasoningEffort(threadId, model));
  // A draft's pick belongs to the draft's model. A thread remembers one per
  // model, which the engine applies when the thread switches to it.
  const providerFor = (model: string) => (perDraft
    ? (model === modelValue ? resolveProvider(threadId) : null)
    : resolveActiveThreadProvider(threadId, model));

  /** Where a pick lands. The model and effort never reach the account
   *  preference: that is a Settings default, and writing it here would leak
   *  the pick to every other draft and thread.
   *
   *  The backend binds here too, and is ALSO remembered on the model's row,
   *  since the last provider picked for a model is remembered per model. */
  function applyPick(patch: ModelSelectionPatch) {
    const write = {
      model: patch.model,
      // `undefined` is the Default row, dropping this draft's own tier.
      reasoningEffort: patch.reasoningEffort ?? undefined,
      // `undefined` clears a pick the new model cannot use.
      provider: patch.provider ?? undefined,
    };
    if (perDraft) updateComposeSelection(threadId ?? null, write);
    else if (threadId) patchThreadModelOverride(threadId, write);
    if (patch.provider) void rememberProvider(patch.model, patch.provider);
  }

  const selection = useModelSelection({
    models: lucidosModelChoices(modelValue),
    vocabulary: LUCIDOS_TIER_VOCABULARY,
    model: modelValue,
    effort: effortValue,
    providerFor,
    effortFor,
    unsetEffortLabel: PROVIDER_DEFAULT_EFFORT_LABEL,
    // A draft can drop its own pick. A thread cannot: its memory would win.
    defaultEffortLabel: perDraft ? (m) => defaultTierLabel(accountChatEffort(m)) : undefined,
    onChange: applyPick,
  });

  function pick(encoded: string, provider?: string) {
    selection.pick(encoded, provider);
    showToast(`Model: ${pairLabelOf(selection.rows, encoded, provider)}`, 'success');
    close();
  }

  return (
    <div class="control-menu" data-row-item ref={menuRef}>
      <ControlMenuTrigger label={LUCIDOS_MENU_LABEL} extraClass={LUCIDOS_TRIGGER_CLASS} onClick={(e) => {
        if (open.value) { close(); return; }
        if (pendingOpen.takeBack()) return;
        openPicker(e.currentTarget as HTMLElement);
      }}>
        <LucidosMenuIcon />
      </ControlMenuTrigger>
      <Overlay
        open={open.value}
        onClose={close}
        anchor={menuRef.current}
        backdrop={false}
        // Portaled for the same reason as the coding-agent menu's panel.
        portal
        panelClass="surface-box control-dropdown control-dropdown-auto"
        panelRef={panelRef}
        panelStyle={anchoredPanelStyle(pos)}
      >
        <ModelSelectionPicker
          label={LUCIDOS_AGENT_LABEL}
          selection={selection}
          keyboardHolder={keyboardHolderRef.current}
          onPick={pick}
        />
      </Overlay>
    </div>
  );
}
