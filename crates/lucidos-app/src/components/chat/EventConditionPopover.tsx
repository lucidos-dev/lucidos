import { useEffect, useRef } from 'preact/hooks';
import { eventConditionPopover } from '../../store/store';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { eventConditionBody, eventConditionTitle } from './eventConditionBody';

function close() {
  eventConditionPopover.value = null;
}

export function EventConditionPopover() {
  const open = eventConditionPopover.value;
  const panelRef = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(open?.anchor ?? null, panelRef, '.thread-pane');
  const placed = pos !== null;

  useEffect(() => {
    if (placed) panelRef.current?.querySelector<HTMLButtonElement>('[data-role="surface-close"]')?.focus();
  }, [placed]);

  if (!open) return null;

  return (
    <Overlay
      open
      onClose={close}
      anchor={open.anchor}
      backdrop={false}
      portal
      panelClass="surface anchored-popover event-condition-popover"
      panelStyle={pos
        ? { top: `${pos.top}px`, left: `${pos.left}px`, '--anchored-popover-fit': `${pos.maxWidth}px` }
        : { visibility: 'hidden' }}
      panelRole="dialog"
      ariaModal
      panelRef={panelRef}
      dataRole="event-condition-popover"
      panelProps={{ 'aria-label': `Condition on ${open.eventType}` }}
    >
      <SurfaceHead title={eventConditionTitle(open)} onClose={close} closeLabel="Close condition" />
      <div class="surface-body anchored-popover-body event-condition-popover-body" tabIndex={-1}>
        {eventConditionBody(open)}
      </div>
    </Overlay>
  );
}
