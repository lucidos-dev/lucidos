import { useState, useRef } from 'preact/hooks';
import { Overlay } from './Overlay';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { MoreIcon, CopyIcon, DownloadIcon, ArchiveIcon, InfoIcon } from './icons';
import { copyThreadRef, copyThreadTitle } from '../../utils/threadRef';
import { exportThread } from '../../utils/exportThread';
import { resolveThreadActions } from '../../store/actions/threadActions';
import { threadMap, effectiveThreadStatus } from '../../store/store';
import { threadInfoRows } from '../drawer/threadRowInfo';

/** Per-thread overflow (⋯) menu — a sleek icon-list popover: a conditional
 *  Archive action first, then Copy thread reference / Copy thread title /
 *  Download thread, then a conditional Info row last. Built on the central
 *  <Overlay> (full dismiss/Escape/inert contract) as a `portal`ed,
 *  `position: fixed` anchored popover so it escapes the drawer's scroll/overflow
 *  clipping and any transformed header ancestor.
 *
 *  The Info item opens a second anchored popover with the thread's structured
 *  details (Status / You / Agent / Type / Exchanges / Started) — the same rows
 *  that used to ride the drawer row's hover tooltip, now reachable everywhere
 *  the ⋯ menu lives (drawer row + both thread-title headers).
 *
 *  `stopPropagation` guards a host whose container has its own click handler (the
 *  drawer row's focus-thread `onClick`) — toggling the menu must not also fire it.
 */
export function ThreadOverflowMenu({ threadId, title, stopPropagation, extraClass }: {
  threadId: string;
  title: string;
  stopPropagation?: boolean;
  extraClass?: string;
}) {
  const triggerRef = useRef<HTMLButtonElement>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const infoRef = useRef<HTMLDivElement>(null);
  // Anchor element when open, null when closed — `useAnchoredPosition` reacts to
  // anchor changes via its effect deps, so no separate `open` flag is needed.
  // The menu and the Info popover each carry their own anchor; they're mutually
  // exclusive (opening one closes the other).
  const [anchor, setAnchor] = useState<HTMLElement | null>(null);
  const [infoAnchor, setInfoAnchor] = useState<HTMLElement | null>(null);
  const open = anchor !== null;
  const infoOpen = infoAnchor !== null;
  // Right-align both popovers to the ⋯ trigger: the trigger sits at the far right
  // of a drawer row / thread-title header, so left-start placement would push the
  // wide panel off-screen and the viewport clamp would strand it near the left
  // edge (the "all the way to the left" report on narrow mobile viewports).
  const pos = useAnchoredPosition(anchor, menuRef, undefined, 'end');
  const infoPos = useAnchoredPosition(infoAnchor, infoRef, undefined, 'end');

  const close = () => setAnchor(null);
  const closeInfo = () => setInfoAnchor(null);
  const toggle = (e: MouseEvent) => {
    if (stopPropagation) e.stopPropagation();
    closeInfo();
    setAnchor(open ? null : triggerRef.current);
  };
  const run = (fn: () => void) => (e: MouseEvent) => {
    if (stopPropagation) e.stopPropagation();
    close();
    fn();
  };

  // Thread meta drives the Info rows; read ONLY while a popover is open so a
  // closed menu subscribes to no signals — preserving the drawer's per-row render
  // budget (threadMap is a hot signal). An unhydrated search hit has no live
  // thread, so `infoRows` is null and the Info item is omitted.
  const liveThread = (open || infoOpen) ? threadMap.value.get(threadId) : undefined;
  const infoRows = liveThread ? threadInfoRows(liveThread.meta, effectiveThreadStatus(liveThread)) : null;
  // Archive availability comes from the canonical selector, read ONLY while the
  // menu is open (resolveThreadActions reads threadMap/changes signals).
  const archiveAction = open
    ? resolveThreadActions(threadId).find((a) => a.kind === 'archive')
    : undefined;

  return (
    <>
      <button
        ref={triggerRef}
        type="button"
        class={`icon-btn header-icon${extraClass ? ` ${extraClass}` : ''}`}
        onClick={toggle}
        aria-haspopup="menu"
        aria-expanded={open}
        aria-label="More thread actions"
        data-tooltip="More actions"
      >
        <MoreIcon />
      </button>
      <Overlay
        open={open}
        onClose={close}
        anchor={triggerRef.current}
        backdrop={false}
        portal
        panelClass="thread-overflow-menu"
        panelRole="menu"
        panelRef={menuRef}
        panelStyle={pos
          ? { position: 'fixed', top: `${pos.top}px`, left: `${pos.left}px` }
          : { visibility: 'hidden' }}
      >
        {archiveAction && (
          <>
            <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void archiveAction.invoke(); })}>
              <ArchiveIcon />
              Archive
            </button>
            <div class="thread-overflow-divider" role="separator" />
          </>
        )}
        <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => copyThreadRef(threadId, title))}>
          <CopyIcon />
          Copy thread reference
        </button>
        <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => copyThreadTitle(title))}>
          <CopyIcon />
          Copy thread title
        </button>
        <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => { void exportThread(threadId, title); })}>
          <DownloadIcon />
          Download thread
        </button>
        {infoRows && (
          <>
            <div class="thread-overflow-divider" role="separator" />
            <button type="button" class="thread-overflow-item" role="menuitem" onClick={run(() => setInfoAnchor(triggerRef.current))}>
              <InfoIcon />
              Info
            </button>
          </>
        )}
      </Overlay>
      <Overlay
        open={infoOpen}
        onClose={closeInfo}
        anchor={triggerRef.current}
        backdrop={false}
        portal
        panelClass="thread-info-popover"
        panelRef={infoRef}
        panelStyle={infoPos
          ? { position: 'fixed', top: `${infoPos.top}px`, left: `${infoPos.left}px` }
          : { visibility: 'hidden' }}
      >
        {infoRows && (
          <div class="thread-info-rows">
            {infoRows.map((r) => (
              <div class="thread-info-row" key={r.label}>
                <span class="thread-info-label">{r.label}</span>
                <span class="thread-info-value">
                  {r.tone && <span class={`thread-info-dot thread-info-dot-${r.tone}`} />}
                  {r.value}
                </span>
              </div>
            ))}
          </div>
        )}
      </Overlay>
    </>
  );
}
