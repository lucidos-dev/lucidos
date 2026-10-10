import type { ComponentChildren } from 'preact';
import type { ToastType } from '../../store/types';
import { BackIcon, CloseIcon, ToneIcon } from './icons';

/** The drill-in's way back to the view it came from, named after that view. */
export interface SurfaceBack {
  label: string;
  onClick: () => void;
}

/** A surface's tone, as the only coloured mark on it (ADR 0290). */
export function SurfaceToneIcon({ tone }: { tone: ToastType }) {
  return (
    <span class={`surface-icon surface-icon-${tone}`} aria-hidden="true"><ToneIcon tone={tone} /></span>
  );
}

/** The head row every surface shares (styles/global/surface.css): an optional
 *  back link, a glyph, the title, a meta readout, actions, and the close X.
 *  The X is the ONE close affordance a surface has, so a surface never grows a
 *  text Close button at its foot. */
export function SurfaceHead({
  title,
  icon,
  meta,
  actions,
  back,
  onClose,
  closeLabel = 'Close',
}: {
  title?: ComponentChildren;
  /** A leading glyph: a `SurfaceToneIcon`, or the spinner of work in flight. */
  icon?: ComponentChildren;
  meta?: ComponentChildren;
  /** Controls that act on what the surface shows, before the X. */
  actions?: ComponentChildren;
  back?: SurfaceBack;
  onClose?: () => void;
  closeLabel?: string;
}) {
  return (
    <div class="surface-head">
      {back ? (
        <button type="button" class="surface-back" data-role="surface-back" onClick={back.onClick}>
          <BackIcon />
          {back.label}
        </button>
      ) : null}
      {icon ?? null}
      <div class="surface-title">{title}</div>
      {meta ? <span class="surface-meta">{meta}</span> : null}
      {actions ? <div class="surface-actions">{actions}</div> : null}
      {onClose ? (
        <button
          type="button"
          class="icon-btn surface-close"
          data-role="surface-close"
          aria-label={closeLabel}
          data-tooltip={closeLabel}
          onClick={onClose}
        >
          <CloseIcon />
        </button>
      ) : null}
    </div>
  );
}
