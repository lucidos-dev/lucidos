import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ThreadWidget } from '../../api/client/widgets';
import { OverflowMenu, type OverflowMenuOpener } from '../shared/OverflowMenu';
import { Disclosure } from '../shared/Disclosure';
import { Overlay } from '../shared/Overlay';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { ChevronDownIcon, CloseIcon, WidgetIcon } from '../shared/icons';
import { useLongPress } from '../../hooks/useLongPress';
import { viewportIsMobile } from '../../utils/viewport';
import { openShelfWidget, threadWidgetsFor } from '../../store/widgets';
import { closeShelfWidget, rereadThreadWidgets } from '../../store/actions/widgets';
import { showWidgetInThread, toggleShelfWidget } from '../../store/actions/widget-actions';
import { WidgetBar } from './WidgetCard';
import { WidgetFrame } from './WidgetFrame';
import { widgetMenuItems, type WidgetMenuTarget } from './WidgetMenu';

function menuTarget(threadId: string, entry: ThreadWidget): WidgetMenuTarget {
  return {
    threadId,
    appId: entry.app_id,
    name: entry.name,
    reusable: entry.reusable,
    pinned: entry.pinned,
    shownEventId: entry.shown_event_id,
  };
}

/** One chip. A tap drops the widget open under the title; a right-click or a
 *  long press opens its menu. The chip draws no ⋯ of its own: the open
 *  widget's bar has one. */
function ShelfChip({ threadId, entry, open }: { threadId: string; entry: ThreadWidget; open: boolean }) {
  const openMenu = useRef<OverflowMenuOpener | null>(null);
  const press = useLongPress(
    (chip, at) => openMenu.current?.(chip, viewportIsMobile.value ? undefined : at),
    () => toggleShelfWidget(threadId, entry.app_id),
  );
  return (
    <>
      <div
        role="button"
        tabIndex={0}
        class={`widget-chip${open ? ' is-open' : ''}`}
        data-widget-chip={entry.app_id}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={`${entry.name} widget`}
        onPointerDown={press.onPointerDown}
        onPointerMove={press.onPointerMove}
        onPointerUp={press.onPointerUp}
        onPointerLeave={press.onPointerLeave}
        onPointerCancel={press.onPointerCancel}
        onContextMenu={(e) => { if (!e.altKey) press.onContextMenu(e); }}
        onClick={press.onClick}
        onKeyDown={(e) => {
          if (e.key !== 'Enter' && e.key !== ' ') return;
          e.preventDefault();
          toggleShelfWidget(threadId, entry.app_id);
        }}
      >
        <WidgetIcon />
        <span class="widget-chip-name">{entry.name}</span>
      </div>
      <OverflowMenu
        ariaLabel={`${entry.name} widget actions`}
        hostOpener={{ ref: openMenu, trigger: false }}
        items={(ctx) => widgetMenuItems(ctx, menuTarget(threadId, entry))}
      />
    </>
  );
}

/** The widget shelf in a thread's title row (ADRs 0402, 0407): a chip per
 *  pinned widget, and the widget a chip dropped open, under the title. Every
 *  widget also shows as a card in the transcript, so an unpinned one has no
 *  chip and loses nothing.
 *
 *  The dropped open widget covers the transcript, so it is an `<Overlay>`
 *  anchored to its chip: an outside tap or Escape closes it. It rolls open
 *  inside that overlay, and the overlay fades it out. */
export function WidgetShelf({ threadId }: { threadId: string }) {
  const [shelfEl, setShelfEl] = useState<HTMLDivElement | null>(null);
  const panelRef = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (threadWidgetsFor(threadId).status === 'not-loaded') rereadThreadWidgets(threadId);
  }, [threadId]);

  const widgets = threadWidgetsFor(threadId);
  // A shelf is extra chrome on a title row: nothing shows until the thread's
  // widgets land, and a failed read leaves the row as it was.
  const chips = widgets.status === 'loaded' ? widgets.data.filter((e) => e.pinned) : [];
  const open = openShelfWidget.value;
  const openEntry = open?.threadId === threadId ? chips.find((e) => e.app_id === open.appId) : undefined;
  const chip = openEntry && shelfEl
    ? shelfEl.querySelector<HTMLElement>(`[data-widget-chip="${CSS.escape(openEntry.app_id)}"]`)
    : null;
  // The drop spans the title row, under it.
  const row = openEntry && shelfEl
    ? shelfEl.closest<HTMLElement>('.thread-view-header, .mobile-thread-title-row')
    : null;
  const pos = useAnchoredPosition(row, panelRef);
  const [rowWidth, setRowWidth] = useState(0);
  useLayoutEffect(() => {
    if (!row) return;
    const measure = () => setRowWidth(row.getBoundingClientRect().width);
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(row);
    return () => observer.disconnect();
  }, [row]);
  if (chips.length === 0) return null;

  return (
    <div ref={setShelfEl} class="widget-shelf" role="group" aria-label="Widget shelf">
      {chips.map((entry) => (
        <ShelfChip key={entry.app_id} threadId={threadId} entry={entry} open={entry === openEntry} />
      ))}
      <Overlay
        open={!!openEntry}
        onClose={closeShelfWidget}
        anchor={chip}
        backdrop={false}
        portal
        panelClass="widget-shelf-drop"
        panelRef={panelRef}
        panelStyle={pos && rowWidth
          ? { position: 'fixed', top: `${pos.top}px`, left: `${pos.left}px`, width: `${rowWidth}px` }
          : { visibility: 'hidden' }}
      >
        {openEntry && (
          <Disclosure open appear>
            <OpenShelfWidget threadId={threadId} entry={openEntry} />
          </Disclosure>
        )}
      </Overlay>
    </div>
  );
}

function OpenShelfWidget({ threadId, entry }: { threadId: string; entry: ThreadWidget }) {
  const target = menuTarget(threadId, entry);
  return (
    <div class="widget-card widget-card-shelf">
      <WidgetBar
        target={target}
        leading={(
          <button
            type="button"
            class="icon-btn header-icon"
            aria-label={`Show ${entry.name} in the thread`}
            data-tooltip="Show in thread"
            onClick={() => showWidgetInThread(threadId, entry.shown_event_id)}
          >
            <ChevronDownIcon size="1rem" />
          </button>
        )}
        trailing={(
          <button
            type="button"
            class="icon-btn header-icon"
            aria-label={`Close ${entry.name}`}
            data-tooltip="Close"
            onClick={closeShelfWidget}
          >
            <CloseIcon />
          </button>
        )}
      />
      <WidgetFrame appId={entry.app_id} reveal={entry.reveal} place="shelf" />
    </div>
  );
}
