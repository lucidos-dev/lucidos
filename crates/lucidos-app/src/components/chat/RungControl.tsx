import { useSignal, useSignalEffect } from '@preact/signals';
import type { ComponentChildren } from 'preact';
import { useEffect, useRef } from 'preact/hooks';
import { LUCIDOS_AGENT_LABEL } from '../../store/thread-events';
import { Overlay } from '../shared/Overlay';
import { anchoredPanelStyle, useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { usePendingOpen } from '../../hooks/usePendingOpen';
import { viewportIsMobile } from '../../utils/viewport';
import { keyboardHolder, returnKeyboard } from '../shared/keyboardHandoff';
import { RungPicker, controlMenuParts } from './controlMenuParts';
import { rungFoldOpenRequest, takeControlMenuPress, useControlMenuCloseRequests } from './controlMenuRequests';
import { RungButtons, type RungControlProps } from './RungButtons';
import type { ShownRung } from './rungControlParts';
import { useChatRungState } from './useChatRungState';
import { useRungState } from './useRungState';

function RungShell({ folded, rowAttrs, shown, step, disabledReason, lockedReason, children }: {
  disabledReason?: string | null;
  lockedReason: string | null;
  folded: RungControlProps['folded'];
  rowAttrs: RungControlProps['rowAttrs'];
  shown: ShownRung | null;
  step?: { prev: (() => void) | null; next: (() => void) | null };
  children: (list: { close: () => void; keyboardHolder: HTMLElement | null }) => ComponentChildren;
}) {
  const triggerRef = useRef<HTMLButtonElement | null>(null);
  const foldAnchorRef = useRef<HTMLElement | null>(null);
  const anchorEl = folded ? foldAnchorRef.current : triggerRef.current;
  const panelRef = useRef<HTMLDivElement>(null);
  const keyboardHolderRef = useRef<HTMLElement | null>(null);
  const pendingOpen = usePendingOpen();
  const open = useSignal(false);

  function openList() {
    if (disabledReason) return;
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
    const active = document.activeElement as HTMLElement | null;
    if (active && (triggerRef.current?.contains(active) || panelRef.current?.contains(active))) active.blur();
    open.value = false;
  }

  useEffect(() => close, []);
  useEffect(() => {
    if (disabledReason) {
      pendingOpen.takeBack();
      if (open.value) close();
    }
  }, [disabledReason]);

  useEffect(() => {
    if (takeControlMenuPress('rung')) openList();
  }, []);

  const wasFolded = useRef(folded);
  useEffect(() => {
    if (wasFolded.current !== folded) close();
    wasFolded.current = folded;
  }, [folded]);
  useControlMenuCloseRequests(open, close);

  useSignalEffect(() => {
    const anchor = rungFoldOpenRequest.value;
    if (!folded || !anchor) return;
    rungFoldOpenRequest.value = null;
    foldAnchorRef.current = anchor;
    openList();
  });

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
          disabledReason={disabledReason}
          lockedReason={lockedReason}
          rowAttrs={rowAttrs}
          expanded={open.value}
          triggerRef={(el) => { triggerRef.current = el; }}
          onOpen={() => {
            if (open.value) { close(); return; }
            if (pendingOpen.takeBack()) return;
            openList();
          }}
          step={step}
        />
      )}
      <Overlay
        open={open.value}
        onClose={close}
        anchor={anchorEl}
        backdrop={false}
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

function CodingAgentRungControl({ folded, rowAttrs, threadId, composeThreadId, codingAgent, disabledReason }: RungControlProps) {
  const rung = useRungState({ threadId, composeThreadId, codingAgent });
  return (
    <RungShell
      folded={folded}
      rowAttrs={rowAttrs}
      shown={rung.shown}
      disabledReason={disabledReason ?? (rung.sending.value ? 'Changing the model and effort' : null)}
      lockedReason={rung.lockedReason}
      step={{
        prev: rung.steps.prev === null ? null : () => rung.step(-1),
        next: rung.steps.next === null ? null : () => rung.step(1),
      }}
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

function ChatRungControl({ folded, rowAttrs, threadId, composeContext, disabledReason }: RungControlProps) {
  const chat = useChatRungState({ threadId, composeContext: composeContext === true });
  return (
    <RungShell
      folded={folded}
      rowAttrs={rowAttrs}
      shown={chat.shown}
      disabledReason={disabledReason}
      lockedReason={chat.lockedReason}
      step={{
        prev: chat.steps.prev === null ? null : () => chat.step(-1),
        next: chat.steps.next === null ? null : () => chat.step(1),
      }}
    >
      {({ close, keyboardHolder: holder }) => (
        <>
          {chat.shown?.nextTurn && <div class="rung-turn-summary" role="status">
            <div>Running: {chat.shown.name}{chat.shown.effort ? ` · ${chat.shown.effort}` : ''}</div>
            <div>* Next turn: {chat.shown.nextTurn}</div>
          </div>}
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
        </>
      )}
    </RungShell>
  );
}

export function RungControl(props: RungControlProps) {
  return props.codingAgent ? <CodingAgentRungControl {...props} /> : <ChatRungControl {...props} />;
}
