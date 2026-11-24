import { Fragment } from 'preact';
import type { ComponentChild, ComponentChildren } from 'preact';
import { OverflowMenu, type OverflowMenuContext } from '../shared/OverflowMenu';
import { keepFocusOnPress } from '../../utils/dom';
import { ToggleIconButton, type ToggleTone } from '../shared/ToggleIconButton';

export interface HeaderActionSpec {
  key: string;
  label: string;
  icon: () => ComponentChild;
  onClick?: (e: MouseEvent) => void;
  tooltip?: string;
  href?: string | null;
  extraClass?: string;
  dataRole?: string;
  active?: boolean;
  tone?: ToggleTone;
  keepsFocus?: boolean;
  disabledTooltip?: string;
  render?: (attrs: Record<string, string>) => ComponentChild;
  onMenuClick?: (anchor: HTMLElement | null) => void;
}

export function renderHeaderAction(a: HeaderActionSpec, attrs: Record<string, string> = {}): ComponentChild {
  const rowAttrs = a.dataRole ? { ...attrs, 'data-role': a.dataRole } : attrs;
  if (a.render) return a.render(rowAttrs);
  const sizeClass = `header-icon${a.extraClass ? ` ${a.extraClass}` : ''}`;
  const cls = `icon-btn ${sizeClass}`;
  const tooltip = a.tooltip ?? a.label;
  if (a.href !== undefined) {
    return (
      <a {...rowAttrs} class={cls} href={a.href ?? undefined} target="_blank" rel="noopener noreferrer" aria-label={a.label} data-tooltip={tooltip}>
        {a.icon()}
      </a>
    );
  }
  if (a.disabledTooltip) {
    return (
      <button {...rowAttrs} class={cls} disabled aria-label={a.disabledTooltip} data-tooltip={a.disabledTooltip} style="pointer-events: auto;">
        {a.icon()}
      </button>
    );
  }
  const buttonProps = {
    ...rowAttrs,
    onClick: a.onClick,
    onMouseDown: a.keepsFocus ? keepFocusOnPress : undefined,
    'aria-label': a.label,
    'data-tooltip': tooltip,
  };
  if (a.active !== undefined) {
    return (
      <ToggleIconButton {...buttonProps} class={sizeClass} pressed={a.active} tone={a.tone}>
        {a.icon()}
      </ToggleIconButton>
    );
  }
  return (
    <button {...buttonProps} class={cls}>
      {a.icon()}
    </button>
  );
}

function menuActionLabel(label: string): ComponentChild {
  return <span class="thread-overflow-label">{label}</span>;
}

export function renderMenuAction(a: HeaderActionSpec, ctx: OverflowMenuContext): ComponentChildren {
  const cls = `thread-overflow-item${a.extraClass ? ` ${a.extraClass}` : ''}`;
  const role = a.dataRole;
  if (a.href !== undefined) {
    return (
      <a key={a.key} class={cls} data-role={role} role="menuitem" href={a.href ?? undefined} target="_blank" rel="noopener noreferrer" onClick={ctx.run(() => {})}>
        {a.icon()}
        {menuActionLabel(a.label)}
      </a>
    );
  }
  if (a.disabledTooltip) {
    return (
      <button key={a.key} type="button" class={cls} data-role={role} role="menuitem" aria-disabled="true" data-tooltip={a.disabledTooltip}>
        {a.icon()}
        {menuActionLabel(a.label)}
      </button>
    );
  }
  const toggle = a.active !== undefined;
  return (
    <button
      key={a.key}
      type="button"
      class={cls}
      data-role={role}
      role={toggle ? 'menuitemcheckbox' : 'menuitem'}
      aria-checked={a.active}
      onClick={(e: MouseEvent) => ctx.run(
        a.onMenuClick ? () => a.onMenuClick!(ctx.anchor) : () => a.onClick?.(e),
      )(e)}
    >
      {a.icon()}
      {menuActionLabel(a.label)}
    </button>
  );
}

export function CollapsingActions({ actions, collapsed, moreClass, itemAttrs, onMenuOpen, children }: {
  actions: readonly HeaderActionSpec[];
  collapsed: number;
  moreClass: string;
  onMenuOpen?: () => void;
  itemAttrs?: Record<string, string>;
  children?: ComponentChildren;
}) {
  const hidden = actions.slice(0, collapsed);
  const visible = actions.slice(collapsed);
  return (
    <>
      {hidden.length > 0 && (
        <OverflowMenu
          ariaLabel="More actions"
          extraClass={moreClass}
          triggerAttrs={itemAttrs}
          onOpen={onMenuOpen}
          items={(ctx) => hidden.map((a) => renderMenuAction(a, ctx))}
        />
      )}
      {visible.map((a) => <Fragment key={a.key}>{renderHeaderAction(a, itemAttrs)}</Fragment>)}
      {children}
    </>
  );
}
