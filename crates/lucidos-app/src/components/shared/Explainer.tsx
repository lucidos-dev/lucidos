import { useState, useRef, useEffect } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { InfoIcon } from './icons';
import { Overlay } from './Overlay';
import { SurfaceHead } from './Surface';
import { protectedClassFrom } from './protectedSurface';

export interface ExplainerProps {
  title: string;
  children: ComponentChildren | ((close: () => void) => ComponentChildren);
  wide?: boolean;
}

export function Explainer({ title, children, wide = false }: ExplainerProps) {
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const [open, setOpen] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  const pos = useAnchoredPosition(open ? anchor : null, panelRef, '.pane, .thread-drawer');
  const placed = pos !== null;

  useEffect(() => {
    if (placed) panelRef.current?.querySelector<HTMLButtonElement>('[data-role="surface-close"]')?.focus();
  }, [placed]);

  const popover = (
    <Overlay
      open={open}
      onClose={() => setOpen(false)}
      anchor={anchor}
      backdrop={false}
      portal
      panelClass={`surface anchored-popover explainer-popover${wide ? ' explainer-popover-wide' : ''}${protectedClassFrom(anchor)}`}
      panelStyle={pos
        ? { top: `${pos.top}px`, left: `${pos.left}px`, '--anchored-popover-fit': `${pos.maxWidth}px` }
        : { visibility: 'hidden' }}
      panelRole="dialog"
      ariaModal
      panelRef={panelRef}
      panelProps={{ 'aria-label': title }}
    >
      <SurfaceHead title={title} onClose={() => setOpen(false)} closeLabel={`Close ${title}`} />
      <div class="surface-body anchored-popover-body explainer-body" tabIndex={-1}>
        {typeof children === 'function' ? children(() => setOpen(false)) : children}
      </div>
    </Overlay>
  );

  return (
    <>
      <span class="explainer-slot">
        <button
          ref={setAnchor}
          type="button"
          class="icon-btn explainer-btn"
          aria-label={`About ${title}`}
          aria-haspopup="dialog"
          aria-expanded={open}
          onClick={() => setOpen((v) => !v)}
        >
          <InfoIcon />
        </button>
      </span>
      {popover}
    </>
  );
}

export function FieldLabel({
  title,
  label,
  children,
}: {
  title: string;
  label?: ComponentChildren;
  children?: ComponentChildren;
}) {
  return (
    <div class="form-label-row">
      <label>{label ?? title}</label>
      {children ? <Explainer title={title}>{children}</Explainer> : null}
    </div>
  );
}
