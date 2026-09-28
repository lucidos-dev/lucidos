import { useState, useRef, useEffect } from 'preact/hooks';
import type { ComponentChildren } from 'preact';
import { useAnchoredPosition } from '../../hooks/useAnchoredPopover';
import { InfoIcon } from './icons';
import { Overlay } from './Overlay';
import { SurfaceHead } from './Surface';
import { trapDialogTab } from './dialogFocusTrap';
import { dialogOwnsKey } from './dialogKeyScope';
import { protectedClassFrom } from './protectedSurface';

export interface ExplainerProps {
  /** What is being explained, as the user sees it named on screen: the control's
   *  own label or its section title. Used verbatim as the popover heading AND as
   *  the button's accessible name ("About <title>"), so the two can't drift. */
  title: string;
  /** The explanation. JSX rather than a string, deliberately: the copy this
   *  replaces carries `<code>`, `<strong>` and the occasional `.accent-link`
   *  button that navigates, none of which survive a plain string. Write it as
   *  `<p>` blocks; `.explainer-body` styles them, plus `code` / `strong` /
   *  `ul` / `li`. (This is also why it does NOT reuse `<DialogMessage>`, whose
   *  contract is one string with blank lines for paragraphs.)
   *
   *  Pass a FUNCTION when the copy contains a link that navigates: it receives
   *  a `close` to call alongside the navigation, without which the popover would
   *  be left floating over the view it just sent the reader to. Nothing else
   *  closes it there, since a click inside the panel is not an outside click. */
  children: ComponentChildren | ((close: () => void) => ComponentChildren);
}

/**
 * The **explainer**: an info icon beside a control, opening a popover that says
 * what the control does. One shared affordance replacing the paragraphs of
 * muted prose that used to sit permanently under the controls they describe
 * (see `docs/plans/2026-08-09-shared-explainer-info-icon.md`, and `explainer`
 * in `docs/glossary.md`).
 *
 * **Where it goes: at the SCOPE of what it explains.** Copy about one control
 * hangs on that control's own label. A section title takes the icon only when
 * nothing narrower owns the copy: a list of rows with no label between them
 * (Repositories, Marketplaces, Connect URLs, Workspace data), a body with no
 * labelled row at all (Network access), or several rows one explanation covers
 * at once (Chat & triggers, whose copy is about Model AND Reasoning together).
 * A heading icon beside a label icon is therefore fine when the two really do
 * explain different-sized things, and it is what the Models page shows.
 *
 * What a reader reads as an inconsistency is the **category error**: two
 * sections of the SAME shape answering it differently. Appearance & Behavior
 * had exactly that. Notifications and Links are both one-control sections, and
 * the icon sat on the Notifications heading while Links carried its own on the
 * row below, so the page looked like it had two unrelated affordances. The push
 * copy is about the switch, so its icon belongs on "Push notifications" and the
 * heading above it stays bare, which is the shape Locale had all along
 * (a "Locale" heading over one explained "Language" row).
 *
 * **It opens at the icon, like the Waits panel.** An anchored popover sits
 * below the icon, or above it when there is no room, clamped inside the pane
 * that holds it. Its width is capped to that pane and its body scrolls, so a
 * narrow drawer pane, a phone's edge and long copy all fit. No scrim dims the
 * app. A tooltip cannot carry it: `data-tooltip` is desktop-only by rule
 * (`.claude/rules/frontend-css.md`).
 *
 * **The icon is glued to the label it explains and can never wrap away from
 * it.** A button is an atomic inline, and line breaking allows a break in front
 * of one. A label squeezed by the control beside it therefore drops the glyph
 * onto a line of its own, under the text, belonging to nothing.
 * `.explainer-slot` is what prevents that: a plain inline box whose `::before`
 * is a U+2060 WORD JOINER, which forbids the break. The joiner is generated
 * content rather than a text node, so it never reaches `textContent`, an
 * accessible name, or a copied selection.
 *
 * The slot must stay `display: inline`. An inline-flex or inline-block one is
 * an atomic inline itself, which hides the joiner from the line breaker and
 * puts the orphan straight back.
 *
 * Built on `<Overlay>`, so click-outside dismiss-and-swallow, the Escape
 * registry and inert-behind all come from the central contract. The icon button
 * is passed as `anchor`: that is what makes a second tap on it close via this
 * component's own handler instead of being raced by the dismiss (with
 * `anchor={null}` a touch reopens the popover on the tap that dismissed it).
 *
 * Host-only. It depends on overlay machinery no app iframe has, so its CSS
 * lives in `styles/global/host-components.css` and is deliberately not served
 * to apps via `shared-components.css`.
 *
 * **The popover is portaled to `<body>`, and that is load-bearing.** An explainer
 * is placed wherever its control is, which includes inside a wrapping
 * `<label>` (every checkbox row: `.thread-filter-option`, `.form-checkbox-row`).
 * A label forwards activation to its control for clicks on any descendant that
 * is not itself interactive content, and a popover's paragraphs are not
 * interactive content, so an inline popover would toggle the checkbox behind it
 * every time the user tapped the explanation. The button itself is safe
 * (interactive content is exempt), which is exactly why the hazard is invisible
 * until someone taps the copy. Portaling moves the popover out from under the
 * label so its clicks bubble through `<body>` instead. The dismiss contract is
 * unaffected: `panel.contains()` works across a portal, and the portaled node
 * sits outside `.app-shell`, so it needs no inert exemption.
 *
 * One accepted limitation: inside a `<fieldset disabled>` the button is
 * disabled along with everything else in the fieldset, so the explanation is
 * unreachable while that section is inert. Un-disabling it is not expressible
 * in HTML, and hand-disabling every control in the fieldset instead (to drop
 * the `disabled` attribute from the fieldset itself) is the worse trade.
 */
export function Explainer({ title, children }: ExplainerProps) {
  // The anchor element, held in state rather than a ref so that resolving it on
  // mount re-renders and hands the real node to <Overlay> (a ref's `.current`
  // is still null on the render that opens).
  const [anchor, setAnchor] = useState<HTMLButtonElement | null>(null);
  const [open, setOpen] = useState(false);
  const panelRef = useRef<HTMLDivElement>(null);
  // Clamped to the pane or drawer holding the icon, so it never spills into
  // the next column. With neither (mobile), the viewport is the box.
  const pos = useAnchoredPosition(open ? anchor : null, panelRef, '.pane, .thread-drawer');
  const placed = pos !== null;

  // Focus goes into the popover once it is placed. Until then it is
  // `visibility: hidden`, and a hidden element takes no focus.
  useEffect(() => {
    if (placed) panelRef.current?.querySelector<HTMLButtonElement>('[data-role="surface-close"]')?.focus();
  }, [placed]);

  // Focus stays in the popover while open, and returns to the icon on close.
  // Required by the `aria-modal` this panel declares: the UI behind is inert to
  // the pointer, but without a trap the keyboard walks straight out into it.
  // Same two shared helpers as `ConfirmDialog`. `dialogOwnsKey` keeps a
  // stacked overlay's Tab from being answered here as well as by its own.
  useEffect(() => {
    if (!open) return;
    const opener = anchor;
    function handleKey(e: KeyboardEvent) {
      if (!dialogOwnsKey(e.target, panelRef.current)) return;
      trapDialogTab(e, panelRef.current);
    }
    document.addEventListener('keydown', handleKey);
    return () => {
      document.removeEventListener('keydown', handleKey);
      // Escape and an outside click both leave focus nowhere useful; put it back
      // on the control the reader was looking at.
      opener?.focus();
    };
  }, [open, anchor]);

  const popover = (
    <Overlay
      open={open}
      onClose={() => setOpen(false)}
      anchor={anchor}
      backdrop={false}
      portal
      panelClass={`surface anchored-popover explainer-popover${protectedClassFrom(anchor)}`}
      // Hidden until the first measurement, so it never flashes at 0,0. The fit
      // is the pane's usable width, which the shell caps the panel to.
      panelStyle={pos
        ? { top: `${pos.top}px`, left: `${pos.left}px`, '--anchored-popover-fit': `${pos.maxWidth}px` }
        : { visibility: 'hidden' }}
      panelRole="dialog"
      ariaModal
      panelRef={panelRef}
      // The visible title is not an accessible name on its own, and an unnamed
      // dialog is announced as just "dialog". `aria-label` rather than
      // `aria-labelledby` because the latter needs an `id`, which this app does
      // not put on dual-rendered components.
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
      {/* The slot keeps the icon on its label's line, per the note above. It
          carries no markup of its own: the word joiner is its `::before`, and
          the gap before the icon is its margin. */}
      <span class="explainer-slot">
        <button
          ref={setAnchor}
          // Never a submit: explainers sit inside real forms (the trigger editor,
          // the credential modal), where the default type would save on click.
          type="button"
          class="icon-btn explainer-btn"
          aria-label={`About ${title}`}
          // The icon stays live beside its open popover, and a second press
          // closes it: the anchor owning its own re-activation is the
          // <Overlay> contract.
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

/**
 * A `.form-group` field label with its explainer beside it, for the form shape
 * where the `<label>` and its control are SIBLINGS (the trigger editor, the
 * credential modal) rather than the label wrapping the control.
 *
 * **The explainer must not go inside that `<label>`, and this component exists
 * to state that once instead of at every field.** A `<button>` is a *labelable*
 * element, and a label with no `for` takes its labeled control from its FIRST
 * labelable descendant. In a wrapping label the checkbox is first and wins, so a
 * checkbox row can hold its explainer inline safely. In a label whose control is
 * a sibling there is no other candidate, so the explainer button becomes the
 * label's control and clicking the label TEXT fires the button: the reader taps
 * "Env var name" expecting to focus the field and a popover opens instead.
 * Keeping the button outside the label leaves the label with no control again,
 * which is what these labels always were.
 */
export function FieldLabel({
  title,
  label,
  children,
}: {
  /** Popover heading, and the label text unless `label` overrides it. */
  title: string;
  /** Label content when it is more than the title, e.g. a trailing
   *  `(optional)` qualifier. Defaults to `title`. */
  label?: ComponentChildren;
  /** The explanation, same contract as `<Explainer>`'s children. Falsy renders
   *  a plain label with no icon, for a field whose explanation only applies in
   *  some states (the trigger editor's Script Path has nothing to say about
   *  runs until the trigger has had some). */
  children?: ComponentChildren;
}) {
  return (
    <div class="form-label-row">
      <label>{label ?? title}</label>
      {children ? <Explainer title={title}>{children}</Explainer> : null}
    </div>
  );
}
