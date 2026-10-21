import { useSignal, useSignalEffect } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { LUCIDOS_AGENT_LABEL } from '../../store/thread-events';
import { showToast } from '../../store/store';
import { Overlay } from '../shared/Overlay';
import { anchoredPanelStyle, useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { usePendingOpen } from '../../hooks/usePendingOpen';
import { viewportIsMobile } from '../../utils/viewport';
import { keyboardHolder, returnKeyboard } from '../shared/keyboardHandoff';
import { RungPicker, controlMenuParts } from './controlMenuParts';
import { rungFoldOpenRequest, takeControlMenuPress } from './controlMenuRequests';
import { RungButtons, type RungControlProps } from './RungButtons';
import type { ShownRung } from './rungControlParts';
import { useChatRungState } from './useChatRungState';
import { useRungState } from './useRungState';

/** The control's trigger and the list it opens in an overlay. What the list
 *  holds, and what a pick means, belong to the thread kind that draws it. */
function RungShell({ folded, rowAttrs, shown, faces, step, lockedReason = null, children }: {
  folded: RungControlProps['folded'];
  rowAttrs: RungControlProps['rowAttrs'];
  shown: ShownRung | null;
  faces: readonly ShownRung[];
  step?: { prev: (() => void) | null; next: (() => void) | null };
  /** Why the rung cannot change now. The list then stays shut and says why. */
  lockedReason?: string | null;
  /** The list, given how to close it and the element that held the keyboard. */
  children: (list: { close: () => void; keyboardHolder: HTMLElement | null }) => ComponentChildren;
}) {
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  // Folded, the ⋯ is the only box left to anchor against.
  const foldAnchorRef = useRef<HTMLElement | null>(null);
  const anchorEl = folded ? foldAnchorRef.current : triggerRef.current;
  const panelRef = useRef<HTMLDivElement>(null);
  // The prompt, when it held the keyboard as the list opened. Closing hands it
  // back, so the prompt never loses focus under a finger.
  const keyboardHolderRef = useRef<HTMLElement | null>(null);
  const pendingOpen = usePendingOpen();
  const open = useSignal(false);

  function openList() {
    if (lockedReason) {
      showToast(lockedReason, 'info');
      return;
    }
    const trigger = folded ? foldAnchorRef.current : triggerRef.current;
    if (!trigger?.isConnected) return;
    const holder = keyboardHolder();
    pendingOpen.open(controlMenuParts, () => {
      keyboardHolderRef.current = holder;
      open.value = true;
    }, trigger);
  }

  function close() {
    returnKeyboard(keyboardHolderRef.current, panelRef.current);
    keyboardHolderRef.current = null;
    // Blur before DOM removal so any focusout-driven header restore fires while
    // the element is still connected.
    const active = document.activeElement as HTMLElement | null;
    if (active && (triggerRef.current?.contains(active) || panelRef.current?.contains(active))) active.blur();
    open.value = false;
  }

  useEffect(() => close, []);

  // A press on the placeholder before this code arrived opens the list as that
  // press would have.
  useEffect(() => {
    if (takeControlMenuPress('rung')) openList();
  }, []);

  // A fold or unfold moves the anchor to another box. An open list is shut
  // then, not left pinned to the box that went.
  const wasFolded = useRef(folded);
  useEffect(() => {
    if (wasFolded.current !== folded) close();
    wasFolded.current = folded;
  }, [folded]);

  // The ⋯ menu row for a folded control asks for the list against the ⋯.
  useSignalEffect(() => {
    const anchor = rungFoldOpenRequest.value;
    if (!folded || !anchor) return;
    rungFoldOpenRequest.value = null;
    foldAnchorRef.current = anchor;
    openList();
  });

  // The list centres on the pill, clamped into the pane when the pill sits
  // too near an edge. Under a finger it centres in the pane, as the menus do.
  const pos = useAnchoredPosition(
    open.value ? anchorEl : null,
    panelRef,
    '.thread-pane',
    viewportIsMobile.value ? 'container-center' : 'center',
  );

  return (
    <>
      {!folded && (
        <RungButtons
          shown={shown}
          faces={faces}
          rowAttrs={rowAttrs}
          expanded={open.value}
          triggerRef={(el) => { triggerRef.current = el; }}
          onOpen={() => {
            if (open.value) { close(); return; }
            if (pendingOpen.takeBack()) return;
            openList();
          }}
          step={step}
          lockedReason={lockedReason}
        />
      )}
      <Overlay
        open={open.value}
        onClose={close}
        anchor={anchorEl}
        backdrop={false}
        // Portaled so it can lay over the header: a mobile swipe pane clips at
        // its own edge and caps any z-index inside it.
        portal
        panelClass="surface-box control-dropdown control-dropdown-auto"
        panelRef={panelRef}
        panelStyle={anchoredPanelStyle(pos)}
      >
        {children({ close, keyboardHolder: keyboardHolderRef.current })}
      </Overlay>
    </>
  );
}

/** A coding-agent thread's control. Its list is the ladder's rungs, and its
 *  steps walk one rung either way.
 *
 *  What a pick means (the pair, per draft, never an account preference) is
 *  `useRungState`'s. */
function CodingAgentRungControl({ folded, rowAttrs, threadId, composeThreadId, codingAgent }: RungControlProps) {
  const rung = useRungState({ threadId, composeThreadId, codingAgent });
  return (
    <RungShell
      folded={folded}
      rowAttrs={rowAttrs}
      shown={rung.shown}
      faces={rung.faces}
      step={{
        prev: rung.steps.prev === null ? null : () => rung.step(-1),
        next: rung.steps.next === null ? null : () => rung.step(1),
      }}
      lockedReason={rung.lockedReason}
    >
      {({ close, keyboardHolder: holder }) => (
        <RungPicker
          label="Model"
          rows={rung.rungList.rows}
          current={rung.rungList.current}
          disabled={rung.sending.value}
          keyboardHolder={holder}
          onPick={(value) => {
            close();
            rung.pickRung(value);
          }}
        />
      )}
    </RungShell>
  );
}

/** A Lucidos Agent thread's control. Its list is the chat ladder's rungs,
 *  and its steps walk one rung either way, as a coding agent's do. A rung
 *  with a choice of backend opens the preferred-provider step. */
function ChatRungControl({ folded, rowAttrs, threadId, composeContext }: RungControlProps) {
  const chat = useChatRungState({ threadId, composeContext: composeContext === true });
  return (
    <RungShell
      folded={folded}
      rowAttrs={rowAttrs}
      shown={chat.shown}
      faces={chat.faces}
      step={{
        prev: chat.steps.prev === null ? null : () => chat.step(-1),
        next: chat.steps.next === null ? null : () => chat.step(1),
      }}
    >
      {({ close, keyboardHolder: holder }) => (
        <RungPicker
          label={LUCIDOS_AGENT_LABEL}
          rows={chat.rungList.rows}
          current={chat.rungList.current}
          keyboardHolder={holder}
          providerStep={chat.providerStep}
          onPick={(value, provider) => {
            close();
            chat.pickRung(value, provider);
          }}
        />
      )}
    </RungShell>
  );
}

/** The prompt bar's rung control: a pill beside the send button, with a step
 *  at each end. Its trigger opens the list in an overlay. */
export function RungControl(props: RungControlProps) {
  return props.codingAgent ? <CodingAgentRungControl {...props} /> : <ChatRungControl {...props} />;
}
