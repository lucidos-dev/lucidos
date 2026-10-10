import type { ComponentChildren, VNode } from 'preact';
import { useState } from 'preact/hooks';
import { Disclosure } from '../shared/Disclosure';
import { ChevronRightIcon } from '../shared/icons';
import { formatMessageTimestamp } from '../../utils/formatTime';

/** The top of every plugin panel and receipt: what is happening, then which
 *  plugin. A receipt passes `receiptAt`, which turns the status into a badge
 *  beside the time it happened. A confirm panel passes its buttons as
 *  `actions`: they sit top right, and wrap above the text on a narrow pane.
 *  It holds no hooks, so it is a plain function. */
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

/** A panel section whose body folds away under its heading. The note and the
 *  footer stay outside the fold: they say what confirming does to the whole
 *  list, so they must read even when it is shut. */
export function PluginSection({
  label,
  count,
  defaultOpen = false,
  tone,
  note,
  footer,
  children,
}: {
  label: string;
  count?: number;
  defaultOpen?: boolean;
  /** `danger` marks a section whose files confirming destroys. */
  tone?: 'danger';
  note?: ComponentChildren;
  /** A control under the note, such as the keep-my-edits switch. */
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
        <span class="plugin-install-label">{label}</span>
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
