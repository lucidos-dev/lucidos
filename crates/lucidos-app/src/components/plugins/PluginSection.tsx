import type { ComponentChildren, VNode } from 'preact';
import { useState } from 'preact/hooks';
import { Disclosure } from '../shared/Disclosure';
import { ChevronRightIcon } from '../shared/icons';
import { formatMessageTimestamp } from '../../utils/formatTime';

export function pluginPanelHeader({ status, receiptAt, name, version, description, actions }: {
  status: string;
  receiptAt?: string;
  name: string;
  version: string;
  description?: string;
  actions?: VNode;
}): VNode {
  return (
    <header class="plugin-install-header">
      <div class="plugin-install-header-text">
        <div class="plugin-install-status">
          {receiptAt ? <span class="panel-receipt-badge">{status}</span> : status}
          {receiptAt && <span class="panel-receipt-time">{formatMessageTimestamp(receiptAt)}</span>}
        </div>
        <h2 class="plugin-install-name">
          {name} <span class="plugin-install-version">v{version}</span>
        </h2>
        {description && <p class="plugin-install-description">{description}</p>}
      </div>
      {actions}
    </header>
  );
}

export function PluginSection({
  label,
  detail,
  count,
  defaultOpen = false,
  tone,
  note,
  footer,
  children,
}: {
  label: string;
  detail?: string;
  count?: number;
  defaultOpen?: boolean;
  tone?: 'danger';
  note?: ComponentChildren;
  footer?: ComponentChildren;
  children: ComponentChildren;
}) {
  const [open, setOpen] = useState(defaultOpen);
  return (
    <section class={`plugin-install-section${tone ? ` plugin-install-section-${tone}` : ''}`}>
      <button
        type="button"
        class="plugin-install-section-toggle"
        aria-expanded={open}
        onClick={() => setOpen(!open)}
      >
        <span class="plugin-install-chevron" aria-hidden="true">
          <ChevronRightIcon size="1rem" />
        </span>
        <span class="plugin-install-label">
          {label}
          {detail && <span class="plugin-install-label-detail"> · {detail}</span>}
        </span>
        {count !== undefined && <span class="plugin-install-count">{count}</span>}
      </button>
      <Disclosure open={open}>
        {children}
      </Disclosure>
      {note && <p class="plugin-install-note">{note}</p>}
      {footer}
    </section>
  );
}
