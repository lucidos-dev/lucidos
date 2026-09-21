import { Fragment } from 'preact';
import type { ComponentChild, ComponentChildren } from 'preact';
import { OverflowMenu, type OverflowMenuContext } from '../shared/OverflowMenu';

/** One header action as DATA, so the same record renders either as a full-size
 *  header icon button or as a row inside the collapsed ⋯ overflow menu. `icon`
 *  is a thunk: the header and the menu each need their own vnode.
 *
 *  Shared by the THREE clusters that collapse this way: the content pane's
 *  trailing actions (`ContentHeaderActions`), the thread pane's
 *  (`ThreadHeaderActions`) and the composer row's middle (`PromptInput`).
 *  Anything a cluster never collapses, such as the notifications bell or the
 *  composer's three fixed controls, is not one of these and renders
 *  separately. */
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
  /** `data-role`, stamped on BOTH renderings for the same reason `extraClass`
   *  is: it is how CSS and the e2e suite address one action, and an attribute
   *  only on the header button disappears the moment the action folds. The
   *  composer's toggles are addressed this way. */
  dataRole?: string;
  /** Toggled-on state, adds `filter-active` (apps/plugins search). */
  active?: boolean;
  /** Class for the on-state, when `filter-active`'s frame is wrong. The
   *  composer's toggles each take their own paint keyed on `data-role`, so they
   *  ask for the bare `active` those rules select on. */
  activeClass?: string;
  /** Disabled with an explanatory tooltip (diff-pinned refresh). */
  disabledTooltip?: string;
  /** The full-size rendering, where the default button cannot express it: a
   *  popover host, or a control carrying its own gesture handlers. It is handed
   *  the row attributes and MUST spread them onto its root element, exactly as
   *  the default rendering does. The menu row is unaffected and still comes
   *  from `label`, `icon` and `onClick`. */
  render?: (attrs: Record<string, string>) => ComponentChild;
  /** Run instead of `onClick` from the ⋯ menu, handed the trigger the menu is
   *  anchored to. An action that OPENS a popover needs it: the menu row it was
   *  clicked on unmounts as the menu closes, so the ⋯ is the only box left to
   *  anchor against. */
  onMenuClick?: (anchor: HTMLElement | null) => void;
  /** The rows this member contributes to the ⋯ menu, when ONE row cannot carry
   *  it. A composite control is several rows folded: the composer's change
   *  split button holds an Apply face and a caret menu, and the caret's actions
   *  would go with the fold. Defaults to the single row the fields above
   *  describe. Each row supplies its own `key`. */
  menuRows?: (ctx: OverflowMenuContext) => ComponentChildren;
}

/** Full-size header rendering.
 *
 *  `attrs` are the host cluster's own per-item attributes, such as the composer
 *  row's `data-row-item` measurement hook. They reach the ⋯ trigger too. So a
 *  folded item and the trigger standing in for it cost the measurement the same
 *  box. */
export function renderHeaderAction(a: HeaderActionSpec, attrs: Record<string, string> = {}): ComponentChild {
  const rowAttrs = a.dataRole ? { ...attrs, 'data-role': a.dataRole } : attrs;
  if (a.render) return a.render(rowAttrs);
  const cls = `icon-btn header-icon${a.extraClass ? ` ${a.extraClass}` : ''}${a.active ? ` ${a.activeClass ?? 'filter-active'}` : ''}`;
  const tooltip = a.tooltip ?? a.label;
  // The spread leads, so the rendering's own class and labels always win over a
  // host's attributes rather than the other way round.
  if (a.href !== undefined) {
    return (
      <a {...rowAttrs} class={cls} href={a.href ?? undefined} target="_blank" rel="noopener noreferrer" aria-label={a.label} data-tooltip={tooltip}>
        {a.icon()}
      </a>
    );
  }
  if (a.disabledTooltip) {
    // Override .icon-btn:disabled { pointer-events: none } so the tooltip
    // (which relies on hover events) can still explain why the button is off.
    return (
      <button {...rowAttrs} class={cls} disabled aria-label={a.disabledTooltip} data-tooltip={a.disabledTooltip} style="pointer-events: auto;">
        {a.icon()}
      </button>
    );
  }
  // A spec that declares an on-state is a TOGGLE, and says so. Colour is the
  // only channel the button itself has, and no screen reader reads it.
  return (
    <button
      {...rowAttrs}
      class={cls}
      onClick={a.onClick}
      aria-label={a.label}
      aria-pressed={a.active}
      data-tooltip={tooltip}
    >
      {a.icon()}
    </button>
  );
}

/** The row's words, in a box that stops after two lines.
 *
 *  A hand-written menu row is a verb and needs none of this. A spec's label is
 *  arbitrary: the composer folds two STATUS readouts in here, and the waiting
 *  one reports the model's own sentence. Wrapped, it filled five lines of a
 *  three-row menu. A span rather than the bare text node, because
 *  `text-overflow` has nothing to act on inside an anonymous flex item. */
function menuActionLabel(label: string): ComponentChild {
  return <span class="thread-overflow-label">{label}</span>;
}

/** Collapsed rendering: a ⋯ menu row with the same label + handler. `ctx.run`
 *  closes the menu before firing (links keep their native navigation, `run`
 *  doesn't preventDefault). */
export function renderMenuAction(a: HeaderActionSpec, ctx: OverflowMenuContext): ComponentChildren {
  if (a.menuRows) return a.menuRows(ctx);
  const cls = `thread-overflow-item${a.extraClass ? ` ${a.extraClass}` : ''}`;
  // `data-role` travels with `extraClass`, for the same reason: a selector that
  // reaches only the header button stops resolving the moment the action folds.
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
    // aria-disabled, NOT the disabled attribute: a disabled <button> can't take
    // focus, and when this row is the FIRST [role="menuitem"] (diff-pinned
    // refresh collapses first) OverflowMenu's keyboard-open would focus a
    // no-op target and strand the arrow-key roving outside the panel. An
    // aria-disabled row stays focusable/perceivable and simply has no onClick.
    return (
      <button key={a.key} type="button" class={cls} data-role={role} role="menuitem" aria-disabled="true" data-tooltip={a.disabledTooltip}>
        {a.icon()}
        {menuActionLabel(a.label)}
      </button>
    );
  }
  // A TOGGLE says so. `active` is the on-state the header button paints, and a
  // menu row has no paint to carry it. So the row takes the one menu role able
  // to hold `aria-checked`. `OverflowMenu` roves it like any other item.
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

/** The ⋯ menu (only while something is collapsed) followed by the actions still
 *  wearing their own icon, in order. The `collapsed` count comes from
 *  `useHeaderActionCollapse` and always names the LEADING actions, the ones
 *  furthest from the cluster's outer edge, so the last thing standing is the
 *  action nearest the edge the user's pointer already lives at.
 *
 *  The host element and its ref stay with the caller: it is what the collapse
 *  measurement observes, and each cluster's host sits in a different row. */
export function CollapsingActions({ actions, collapsed, moreClass, itemAttrs, onMenuOpen, children }: {
  actions: readonly HeaderActionSpec[];
  collapsed: number;
  /** Names this cluster's ⋯ trigger for the e2e suite. */
  moreClass: string;
  /** Run as the ⋯ menu opens. See `OverflowMenu`'s `onOpen`. */
  onMenuOpen?: () => void;
  /** Attributes every in-row member carries, the ⋯ trigger included. The
   *  composer passes its `data-row-item` measurement hook here; the two header
   *  clusters measure differently and pass nothing. */
  itemAttrs?: Record<string, string>;
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
