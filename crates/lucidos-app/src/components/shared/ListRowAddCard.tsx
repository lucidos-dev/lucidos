/**
 * The "+ Add <thing>" row that closes a list: Add Repository, Add Credential,
 * Add Model, Add Environment Variable, Add Trigger, New Group, New App.
 *
 * It is a real `<button>`, not a clickable div. A div carries no tab stop, no
 * `button` role for assistive tech, and no Enter/Space activation, so before
 * this component every one of those seven controls was pointer-only: a keyboard
 * user could tab through the Remove button of every repository and never reach
 * the one control that adds one. `.list-row-add-card` in shared-components.css
 * carries the button resets and the focus ring; see the comment there.
 *
 * Exists so the eighth add card cannot reintroduce the div. Enforced by
 * `__tests__/clickable-control-element-guard.test.ts`.
 */
interface ListRowAddCardProps {
  /** Visible text, and therefore the button's accessible name. */
  label: string;
  onClick: () => void;
}

/* No per-call-site `class` escape hatch: the one place two cards differ is the
   pair in the Triggers view, and `.trigger-add-row > .list-row-add-card` in
   skills.css already sizes them from the parent. */
export function ListRowAddCard({ label, onClick }: ListRowAddCardProps) {
  return (
    <button type="button" class="list-row-add-card" onClick={onClick}>
      {/* Spans, not divs: a <div> inside a <button> is invalid HTML. Both are
          flex items either way, so the layout is unchanged. */}
      <span class="list-row-add-icon">+</span>
      <span class="list-row-add-label">{label}</span>
    </button>
  );
}
