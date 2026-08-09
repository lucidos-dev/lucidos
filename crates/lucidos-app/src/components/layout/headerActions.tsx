import { Fragment } from 'preact';
import type { ComponentChild, ComponentChildren } from 'preact';
import { OverflowMenu, type OverflowMenuContext } from '../shared/OverflowMenu';

/** One header action as DATA, so the same record renders either as a full-size
 *  header icon button or as a row inside the collapsed ⋯ overflow menu. `icon`
 *  is a thunk: the header and the menu each need their own vnode.
 *
 *  Shared by BOTH header clusters that collapse this way, the content pane's
 *  trailing actions (`ContentHeaderActions`) and the thread pane's
 *  (`ThreadHeaderActions`). Anything a cluster never collapses, such as the
 *  notifications bell, is not one of these and renders separately. */
export interface HeaderActionSpec {
  key: string;
  /** aria-label and the ⋯ menu row text, and the tooltip unless one is given. */
  label: string;
  icon: () => ComponentChild;
  onClick?: (e: MouseEvent) => void;
  /** Hover tooltip, when it says more than the label: a keyboard shortcut, or a
   *  sentence too long to be a menu row. Defaults to `label`. */
  tooltip?: string;
  /** Renders an `<a target="_blank">` instead of a button (open-in-tab). */
  href?: string | null;
  /** Extra class(es) naming the ACTION, e.g. `app-fullscreen`. Carries no CSS:
   *  it is how the rest of the app (and the e2e suite) addresses one action, so
   *  it is stamped on BOTH renderings. Progressive collapse decides placement,
   *  and an action must stay findable by the same selector wherever it landed.
   *  A class only on the header button silently disappears the moment a long
   *  title folds the action into the overflow menu. */
  extraClass?: string;
  /** Toggled-on state, adds `filter-active` (apps/plugins search). */
  active?: boolean;
  /** Disabled with an explanatory tooltip (diff-pinned refresh). */
  disabledTooltip?: string;
}

/** Full-size header rendering. */
export function renderHeaderAction(a: HeaderActionSpec): ComponentChild {
  const cls = `icon-btn header-icon${a.extraClass ? ` ${a.extraClass}` : ''}${a.active ? ' filter-active' : ''}`;
  const tooltip = a.tooltip ?? a.label;
  if (a.href !== undefined) {
    return (
      <a class={cls} href={a.href ?? undefined} target="_blank" rel="noopener noreferrer" aria-label={a.label} data-tooltip={tooltip}>
        {a.icon()}
      </a>
    );
  }
  if (a.disabledTooltip) {
    // Override .icon-btn:disabled { pointer-events: none } so the tooltip
    // (which relies on hover events) can still explain why the button is off.
    return (
      <button class={cls} disabled aria-label={a.disabledTooltip} data-tooltip={a.disabledTooltip} style="pointer-events: auto;">
        {a.icon()}
      </button>
    );
  }
  return (
    <button class={cls} onClick={a.onClick} aria-label={a.label} data-tooltip={tooltip}>
      {a.icon()}
    </button>
  );
}

/** Collapsed rendering: a ⋯ menu row with the same label + handler. `ctx.run`
 *  closes the menu before firing (links keep their native navigation, `run`
 *  doesn't preventDefault). */
export function renderMenuAction(a: HeaderActionSpec, ctx: OverflowMenuContext): ComponentChild {
  const cls = `thread-overflow-item${a.extraClass ? ` ${a.extraClass}` : ''}`;
  if (a.href !== undefined) {
    return (
      <a key={a.key} class={cls} role="menuitem" href={a.href ?? undefined} target="_blank" rel="noopener noreferrer" onClick={ctx.run(() => {})}>
        {a.icon()}
        {a.label}
      </a>
    );
  }
  if (a.disabledTooltip) {
    // aria-disabled, NOT the disabled attribute: a disabled <button> can't take
    // focus, and when this row is the FIRST [role="menuitem"] (diff-pinned
    // refresh collapses first) OverflowMenu's keyboard-open would focus a
    // no-op target and strand the arrow-key roving outside the panel. An
    // aria-disabled row stays focusable/perceivable and simply has no onClick.
    return (
      <button key={a.key} type="button" class={cls} role="menuitem" aria-disabled="true" data-tooltip={a.disabledTooltip}>
        {a.icon()}
        {a.label}
      </button>
    );
  }
  return (
    <button key={a.key} type="button" class={cls} role="menuitem" onClick={(e: MouseEvent) => ctx.run(() => a.onClick?.(e))(e)}>
      {a.icon()}
      {a.label}
    </button>
  );
}

/** The ⋯ menu (only while something is collapsed) followed by the actions still
 *  wearing their own icon, in order. The `collapsed` count comes from
 *  `useHeaderActionCollapse` and always names the LEADING actions, the ones
 *  furthest from the cluster's outer edge, so the last thing standing is the
 *  action nearest the edge the user's pointer already lives at.
 *
 *  The host element and its ref stay with the caller: it is what the collapse
 *  measurement observes, and each cluster's host sits in a different row. */
export function CollapsingActions({ actions, collapsed, moreClass, children }: {
  actions: readonly HeaderActionSpec[];
  collapsed: number;
  /** Names this cluster's ⋯ trigger for the e2e suite. */
  moreClass: string;
  /** Rendered after the actions: a cluster's never-collapsed trailing member. */
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
          items={(ctx) => hidden.map((a) => renderMenuAction(a, ctx))}
        />
      )}
      {visible.map((a) => <Fragment key={a.key}>{renderHeaderAction(a)}</Fragment>)}
      {children}
    </>
  );
}
