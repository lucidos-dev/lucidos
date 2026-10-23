import { useEffect, useRef } from 'preact/hooks';
import { eventConditionPopover } from '../../store/store';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { Overlay } from '../shared/Overlay';
import { SurfaceHead } from '../shared/Surface';
import { eventConditionBody, eventConditionTitle } from './eventConditionBody';

function close() {
  eventConditionPopover.value = null;
}

/** The `condition` on one *event subscription*, opened at the transcript row's
 *  "with a condition" chip through `eventConditionDoor`.
 *
 *  That door exists because the note it carries states that a filter is in
 *  play and nothing about what the filter says. That was deliberate: the raw
 *  operator JSON is developer-facing and does not belong on a line read by
 *  whoever is waiting. It belongs one tap away, which is here.
 *
 *  It opens at the chip, like every popover (ADR 0299), clamped into the
 *  thread pane. The waiting panel asks the same door but drills in instead,
 *  since a popover never opens a second layer. */
export function EventConditionPopover() {
  const open = eventConditionPopover.value;
  const panelRef = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(open?.anchor ?? null, panelRef, '.thread-pane');
  const placed = pos !== null;

  // Focus goes into the popover once it is placed. Until then it is
  // `visibility: hidden`, and a hidden element takes no focus.
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
      // Hidden until the first measurement, so it never flashes at 0,0. The fit
      // is the thread pane's usable width, which the shell caps the panel to.
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
