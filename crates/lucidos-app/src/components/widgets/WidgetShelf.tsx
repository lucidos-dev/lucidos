import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import type { ThreadWidget } from '../../api/client/widgets';
import { OverflowMenu, type OverflowMenuOpener } from '../shared/OverflowMenu';
import { Disclosure } from '../shared/Disclosure';
import { Overlay } from '../shared/Overlay';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { ChevronDownIcon, CloseIcon } from '../shared/icons';
import { AppIcon } from '../shared/AppIcon';
import { useLongPress } from '../../hooks/useLongPress';
import { viewportIsMobile } from '../../utils/viewport';
import { openShelfWidget, threadWidgetKey, threadWidgetsFor } from '../../store/widgets';
import { closeShelfWidget, rereadThreadWidgets } from '../../store/actions/widgets';
import { isHomeThread } from '../../store/actions/homeThread';
import { showWidgetInThread, toggleShelfWidget } from '../../store/actions/widget-actions';
import { WidgetBar } from './WidgetCard';
import { WidgetFrame } from './WidgetFrame';
import { widgetMenuItems, type WidgetMenuTarget } from './WidgetMenu';
import { useShelfTight } from './useShelfTight';

/** What a chip calls its instance: the label, else the widget's name. */
function chipName(entry: ThreadWidget): string {
  return entry.label ?? entry.name;
}

export function menuTarget(threadId: string, entry: ThreadWidget): WidgetMenuTarget {
  return {
    threadId,
    appId: entry.app_id,
    params: entry.params,
    label: entry.label,
    name: chipName(entry),
    reusable: entry.reusable,
    originPluginId: entry.origin_plugin_id,
    builtIn: entry.built_in ?? false,
    pinned: entry.pinned,
    shownEventId: entry.shown_event_id,
  };
}

/** One chip: the widget's app icon and its name. A tap drops the widget open
 *  under the title; a right-click or a long press opens its menu. The chip
 *  draws no ⋯ of its own: the open widget's bar has one. The name is always
 *  the tooltip, since a tight shelf hides it. */
function ShelfChip({ threadId, entry, open }: { threadId: string; entry: ThreadWidget; open: boolean }) {
  const openMenu = useRef<OverflowMenuOpener | null>(null);
  const instanceKey = threadWidgetKey(entry);
  const name = chipName(entry);
  const press = useLongPress(
    (chip, at) => openMenu.current?.(chip, viewportIsMobile.value ? undefined : at),
    () => toggleShelfWidget(threadId, instanceKey),
  );
  return (
    <>
      <div
        role="button"
        tabIndex={0}
        class={`widget-chip${open ? ' is-open' : ''}`}
        data-widget-chip={entry.app_id}
        data-widget-instance={instanceKey}
        aria-haspopup="dialog"
        aria-expanded={open}
        aria-label={`${name} widget`}
        data-tooltip={name}
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
          toggleShelfWidget(threadId, instanceKey);
        }}
      >
        <AppIcon appId={entry.app_id} name={entry.name} icon={entry.icon} />
        <span class="widget-chip-name">{name}</span>
      </div>
      <OverflowMenu
        ariaLabel={`${name} widget actions`}
        hostOpener={{ ref: openMenu, trigger: false }}
        items={(ctx) => widgetMenuItems(ctx, menuTarget(threadId, entry))}
      />
    </>
  );
}

/** The widget shelf in a thread's title row (ADRs 0402, 0407, 0415): a chip
 *  per pinned widget instance, and the one a chip dropped open, under the title. Every
 *  widget also shows as a card in the transcript, so an unpinned one has no
 *  chip and loses nothing. Home draws no shelf: its pinned widgets open from
 *  the Home long-press menu as windows (ADR 0419).
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
  const chips = widgets.status === 'loaded' && !isHomeThread(threadId)
    ? widgets.data.filter((e) => e.pinned)
    : [];
  const open = openShelfWidget.value;
  const openEntry = open?.threadId === threadId
    ? chips.find((e) => threadWidgetKey(e) === open.instanceKey)
    : undefined;
  const chip = openEntry && shelfEl
    ? shelfEl.querySelector<HTMLElement>(`[data-widget-instance="${CSS.escape(threadWidgetKey(openEntry))}"]`)
    : null;
  // The drop spans the title row, under it.
  const row = openEntry && shelfEl
    ? shelfEl.closest<HTMLElement>('.thread-view-header, .mobile-thread-title-row')
    : null;
  const pos = useAnchoredPosition(row, panelRef);
  // The labels' width changes only with the names on the shelf.
  const tight = useShelfTight(shelfEl, chips.map((e) => e.name).join('\n'));
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
    <div ref={setShelfEl} class={`widget-shelf${tight ? ' is-tight' : ''}`} role="group" aria-label="Widget shelf">
      {chips.map((entry) => (
        <ShelfChip key={threadWidgetKey(entry)} threadId={threadId} entry={entry} open={entry === openEntry} />
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
            <OpenShelfWidget threadId={threadId} entry={openEntry} onClose={closeShelfWidget} />
          </Disclosure>
        )}
      </Overlay>
    </div>
  );
}

/** Lands on the turn that showed the widget. An instance pinned from an embed
 *  was never shown, so has no turn and draws no button. */
export function ShowInThreadButton({ threadId, name, shownEventId }: {
  threadId: string;
  name: string;
  shownEventId?: string;
}) {
  if (!shownEventId) return null;
  return (
    <button
      type="button"
      class="icon-btn header-icon"
      aria-label={`Show ${name} in the thread`}
      data-tooltip="Show in thread"
      onClick={() => showWidgetInThread(threadId, shownEventId)}
    >
      <ChevronDownIcon size="1rem" />
    </button>
  );
}

/** An open widget on the shelf: its bar over its live frame, under the
 *  title, with its own close. */
export function OpenShelfWidget({ threadId, entry, onClose }: {
  threadId: string;
  entry: ThreadWidget;
  onClose: () => void;
}) {
  const target = menuTarget(threadId, entry);
  return (
    <div class="widget-card widget-card-shelf">
      <WidgetBar
        target={target}
        icon={entry.icon}
        leading={<ShowInThreadButton threadId={threadId} name={target.name} shownEventId={entry.shown_event_id} />}
        trailing={(
          <button
            type="button"
            class="icon-btn header-icon"
            aria-label={`Close ${target.name}`}
            data-tooltip="Close"
            onClick={onClose}
          >
            <CloseIcon />
          </button>
        )}
      />
      <WidgetFrame appId={entry.app_id} params={entry.params} reveal={entry.reveal} place="shelf" />
    </div>
  );
}
