import type { ComponentChildren } from 'preact';
import type { ToastType } from '../../store/types';
import { BackIcon, CloseIcon, ToneIcon } from './icons';

export interface SurfaceBack {
  label: string;
  onClick: () => void;
}

export function SurfaceToneIcon({ tone }: { tone: ToastType }) {
  return (
    <span class={`surface-icon surface-icon-${tone}`} aria-hidden="true"><ToneIcon tone={tone} /></span>
  );
}

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
  icon?: ComponentChildren;
  meta?: ComponentChildren;
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
