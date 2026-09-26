import type { StepOutcome } from '../../store/types';

export function ClaudeIcon() {
  return (
    <svg class="claude-icon" viewBox="0 0 16 16" fill="currentColor">
      <path d="m3.127 10.604 3.135-1.76.053-.153-.053-.085H6.11l-.525-.032-1.791-.048-1.554-.065-1.505-.08-.38-.081L0 7.832l.036-.234.32-.214.455.04 1.009.069 1.513.105 1.097.064 1.626.17h.259l.036-.105-.089-.065-.068-.064-1.566-1.062-1.695-1.121-.887-.646-.48-.327-.243-.306-.104-.67.435-.48.585.04.15.04.593.456 1.267.981 1.654 1.218.242.202.097-.068.012-.049-.109-.181-.9-1.626-.96-1.655-.428-.686-.113-.411a2 2 0 0 1-.068-.484l.496-.674L4.446 0l.662.089.279.242.411.94.666 1.48 1.033 2.014.302.597.162.553.06.17h.105v-.097l.085-1.134.157-1.392.154-1.792.052-.504.25-.605.497-.327.387.186.319.456-.045.294-.19 1.23-.37 1.93-.243 1.29h.142l.161-.16.654-.868 1.097-1.372.484-.545.565-.601.363-.287h.686l.505.751-.226.775-.707.895-.585.759-.839 1.13-.524.904.048.072.125-.012 1.897-.403 1.024-.186 1.223-.21.553.258.06.263-.218.536-1.307.323-1.533.307-2.284.54-.028.02.032.04 1.029.098.44.024h1.077l2.005.15.525.346.315.424-.053.323-.807.411-3.631-.863-.872-.218h-.12v.073l.726.71 1.331 1.202 1.667 1.55.084.383-.214.302-.226-.032-1.464-1.101-.565-.497-1.28-1.077h-.084v.113l.295.432 1.557 2.34.08.718-.112.234-.404.141-.444-.08-.911-1.28-.94-1.44-.759-1.291-.093.053-.448 4.821-.21.246-.484.186-.403-.307-.214-.496.214-.98.258-1.28.21-1.016.19-1.263.112-.42-.008-.028-.092.012-.953 1.307-1.448 1.957-1.146 1.227-.274.109-.477-.247.045-.44.266-.39 1.586-2.018.956-1.25.617-.723-.004-.105h-.036l-4.212 2.736-.75.096-.324-.302.04-.496.154-.162 1.267-.871z"/>
    </svg>
  );
}

export function ReloadIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="20.8 5.6 20.8 10.4 16 10.4"/>
      <polyline points="3.2 18.4 3.2 13.6 8 13.6"/>
      <path d="M5.21 9.6A7.2 7.2 0 0 1 14.28 5.09L20.8 10.4M3.2 13.6l6.52 5.31A7.2 7.2 0 0 0 18.79 14.4"/>
    </svg>
  );
}

export function CloseIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M18 6 6 18"/><path d="M6 6 18 18"/>
    </svg>
  );
}

export function CopyIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <rect x="5.5" y="5.5" width="8" height="8" rx="1.5"/>
      <path d="M3 10.5V3a1.5 1.5 0 0 1 1.5-1.5H10"/>
    </svg>
  );
}

export function DownloadIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <path d="M8 2v8"/>
      <path d="M4.5 6.5L8 10l3.5-3.5"/>
      <path d="M2.5 12.5h11"/>
    </svg>
  );
}

/** "Move to top level": an arrow rising to the top line. */
export function MoveToTopIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <path d="M2.5 2.5h11"/>
      <path d="M8 13.5v-8"/>
      <path d="M4.5 9L8 5.5 11.5 9"/>
    </svg>
  );
}

export function MoreIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor">
      <circle cx="8" cy="3.25" r="1.4" />
      <circle cx="8" cy="8" r="1.4" />
      <circle cx="8" cy="12.75" r="1.4" />
    </svg>
  );
}

export function InfoIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="8" cy="8" r="6.25" />
      <path d="M8 7.25v3.5" />
      <circle cx="8" cy="5" r="0.6" fill="currentColor" stroke="none" />
    </svg>
  );
}

/** A solid warning triangle with its "!" cut out, so the mark shows the
 *  surface behind it in every theme. Solid, because an outlined "!" blurs
 *  into its own stroke at text size. */
export function WarningIcon({ className }: { className?: string }) {
  return (
    <svg class={className} viewBox="0 0 16 16" fill="currentColor" fill-rule="evenodd" aria-hidden="true">
      <path d="M6.7 2.3Q8 .3 9.3 2.3L14.6 12.2Q15.6 14 13.6 14H2.4Q.4 14 1.4 12.2ZM8 5a.9.9 0 0 1 .9.9v3.1a.9.9 0 0 1-1.8 0V5.9A.9.9 0 0 1 8 5ZM8 10.8a1 1 0 1 1 0 2a1 1 0 1 1 0-2Z" />
    </svg>
  );
}

/** A "↳" whose ink fills its viewBox top to bottom. Its CSS sizes the box in
 *  cap units: the stem starts on the cap line, and the bottom two units hang
 *  below the baseline. A font's own "↳" sits wherever its designer put it. */
export function ContinuedIcon({ className }: { className?: string }) {
  return (
    <svg class={className} viewBox="0 0 14 14" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M2.75.75V10.5h9.5M9.5 7.75l2.75 2.75L9.5 13.25" />
    </svg>
  );
}

/** Question mark in a circle. Mirrors InfoIcon's geometry (same circle, same
 *  stroke, same baseline dot) so the two read as one family in a header row. */
export function HelpIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="8" cy="8" r="6.25" />
      <path d="M6.3 6.05a1.75 1.75 0 1 1 2.4 1.62c-.45.18-.7.6-.7 1.08v.35" />
      <circle cx="8" cy="11.4" r="0.6" fill="currentColor" stroke="none" />
    </svg>
  );
}

export function ClearIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="10" /><path d="M15 9l-6 6" /><path d="M9 9l6 6" />
    </svg>
  );
}

// The class is not decoration: this glyph paints far less of its viewBox than
// the icons it sits beside, so the boxes that hold it correct for that by name.
// See the `.trash-icon` rule in styles/global/host-components.css.
export function TrashIcon() {
  return (
    <svg class="trash-icon" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M3 6h18" />
      <path d="M8 6V4h8v2" />
      <path d="M19 6l-1 14H6L5 6" />
      <path d="M10 11v5" />
      <path d="M14 11v5" />
    </svg>
  );
}

export function ChevronLeftIcon({ size = '1.25rem' }: { size?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="16 18 8 12 16 6"/>
    </svg>
  );
}

export function ChevronRightIcon({ size = '1.25rem' }: { size?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="9 6 15 12 9 18"/>
    </svg>
  );
}

export function PinIcon({ filled = false, size, className }: { filled?: boolean; size?: string; className?: string }) {
  return (
    <svg class={className} {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill={filled ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M12 17v5" />
      <path d="M9 10.76a2 2 0 0 1-1.11 1.79l-1.78.9A2 2 0 0 0 5 15.24V17h14v-1.76a2 2 0 0 0-1.11-1.79l-1.78-.9A2 2 0 0 1 15 10.76V6h1a2 2 0 0 0 0-4H8a2 2 0 0 0 0 4h1v4.76z" />
    </svg>
  );
}

export function BellIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M18 8A6 6 0 0 0 6 8c0 7-3 9-3 9h18s-3-2-3-9"/>
      <path d="M13.73 21a2 2 0 0 1-3.46 0"/>
    </svg>
  );
}

/** A handset, for the composer's call toggle. It carries both directions:
 *  colour says whether pressing it places a call or ends one. */
export function CallIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M22 16.92v3a2 2 0 0 1-2.18 2 19.79 19.79 0 0 1-8.63-3.07 19.5 19.5 0 0 1-6-6A19.79 19.79 0 0 1 2.12 4.18 2 2 0 0 1 4.11 2h3a2 2 0 0 1 2 1.72c.13.96.36 1.9.7 2.81a2 2 0 0 1-.45 2.11L8.09 9.91a16 16 0 0 0 6 6l1.27-1.27a2 2 0 0 1 2.11-.45c.9.34 1.85.57 2.81.7A2 2 0 0 1 22 16.92z"/>
    </svg>
  );
}

export function CaptureIcon({ size = '1.5rem' }: { size?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="currentColor">
      <circle cx="12" cy="12" r="10"/>
    </svg>
  );
}

export function ImageIcon({ className }: { className?: string }) {
  return (
    <svg class={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
      <circle cx="8.5" cy="8.5" r="1.5"/>
      <polyline points="21 15 16 10 5 21"/>
    </svg>
  );
}

/** Clock face for the *waiting indicator* (an event wait's countdown). */
export function EventWaitClockIcon({ className }: { className?: string }) {
  return (
    <svg class={className} viewBox="0 0 16 16" fill="none" aria-hidden="true">
      <circle cx="8" cy="8" r="6" stroke="currentColor" stroke-width="1.5" />
      <path d="M8 4.5V8l2.5 1.5" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
    </svg>
  );
}

/** The todo indicator's ONE glyph, for every state it can be in.
 *
 *  A ticked checkbox: two strokes, echoing the ✓ the panel's completed rows
 *  use. Idle, in-progress, waiting and abandoned differ only in COLOR (see
 *  `styles/chat/todo-list.css`), never in shape, so the button keeps saying
 *  "todo list" at a glance.
 *
 *  A denser checklist (two ticked rows plus an open circle) was tried here and
 *  dropped: at the 1.25rem the indicator renders, six strokes crowd into a
 *  smudge, while a box and a tick still read. Keep this glyph sparse. */
export function TodoListIcon({ className }: { className?: string }) {
  return (
    <svg class={className} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="3" y="3" width="18" height="18" rx="2" ry="2"/>
      <polyline points="8 12 11 15 16 9"/>
    </svg>
  );
}

export function CameraIcon({ size = '1rem' }: { size?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M23 19a2 2 0 0 1-2 2H3a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4l2-3h6l2 3h4a2 2 0 0 1 2 2z"/>
      <circle cx="12" cy="13" r="4"/>
    </svg>
  );
}

export function ChevronUpIcon({ size = '1.25rem' }: { size?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="6 15 12 9 18 15"/>
    </svg>
  );
}

export function ChevronDownIcon({ size = '1.25rem' }: { size?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="6 9 12 15 18 9"/>
    </svg>
  );
}

/** A horseshoe MAGNET over the LIVE EDGE: the *standing follow*'s toggle, in the
 *  prompt area. It says stick to the live edge, where the chevron beside it
 *  says go there once. NOT `DownloadIcon`, worn one row above, and NOT
 *  `ChevronDownIcon`, which the scroll button keeps: that one NAVIGATES to
 *  the bottom, this one STAYS there. A labelled menu item can share a mark.
 *  An icon-only toggle cannot.
 *
 *  POLES DOWN, over a line, after both free-floating orientations failed at
 *  this size. A horseshoe is read from its inner void and its two pole bands,
 *  and both close up that small, so the bands are gone.
 *
 *  A DETACHED LINE UNDER AN ARCH IS THE RAINBOW GLYPH, and an open-legged
 *  version of this was reported as one. The shut pole ends tell the two arcs
 *  apart from rainbow bands. Widen the 2-unit gap, or reopen the poles, and
 *  the glyph goes back there.
 *
 *  INK IS 0.750 OF THE BOX on both axes, the fraction this row's other glyphs
 *  land. That is the GEOMETRIC extent, stroke excluded. With the stroke it is
 *  0.833, and so is every neighbour. No inline size either: `.icon-btn` sizes
 *  the svg, and an inline size would override it. */
export function FollowLiveEdgeIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M4 17v-6a8 8 0 0 1 16 0v6h-5v-6a3 3 0 0 0-6 0v6z"/>
      <path d="M3 21h18"/>
    </svg>
  );
}

/** A flag planted on the change: apply it once this thread settles. The
 *  *standing apply*'s toggle, two slots along from the magnet above.
 *
 *  On and off by FILL, the shape `PinIcon` uses. That wants one large closed
 *  path, so the banner is the whole glyph and the pole is a bare stroke.
 *
 *  A flag is a standing instruction left behind: plant it, walk away, and it
 *  is still there when the thread finishes. Three other marks say the wrong
 *  thing. A tick says the apply already happened. A clock says what the
 *  *waiting indicator* beside it says, and both can be lit at once. A bolt is
 *  `TriggerFiredIcon`, whose note records that a bolt cannot be outlined at
 *  this size, so it carries no off state at all.
 *
 *  No inline size: this renders inside `.icon-btn`, which sizes the svg. */
export function StandingApplyIcon({ armed = false }: { armed?: boolean }) {
  return (
    <svg viewBox="0 0 24 24" fill={armed ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M4 15s1-1 4-1 5 2 8 2 4-1 4-1V3s-1 1-4 1-5-2-8-2-4 1-4 1z"/>
      <path d="M4 22v-7"/>
    </svg>
  );
}

/** Two speech bubbles, one tucked behind the other: every message in this
 *  turn, not only the latest. Worn by the response header's full-response
 *  toggle.
 *
 *  It does NOT change with the toggle's state. An unfold/fold pair was tried
 *  and visibly changed size on every click while the body under it moved,
 *  which read as the layout dancing. Brightness answers "is it on" instead
 *  (`.turn-controls` in styles/chat/input-messages.css), as it does for the
 *  steps control beside it.
 *
 *  The back bubble draws only the edge the front one leaves showing, so the
 *  two never cross strokes at the 0.875rem this renders at. */
export function FullResponseIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M16 15.5a2.5 2.5 0 0 1-2.5 2.5H8.5L5 21v-3h.5A2.5 2.5 0 0 1 3 15.5v-4A2.5 2.5 0 0 1 5.5 9h8a2.5 2.5 0 0 1 2.5 2.5z"/>
      <path d="M8 9V5.5A2.5 2.5 0 0 1 10.5 3h8A2.5 2.5 0 0 1 21 5.5v4a2.5 2.5 0 0 1-2.5 2.5H16"/>
    </svg>
  );
}

/** A circled minus to fold this turn, a circled plus to unfold it. Worn by both
 *  turn headers: the third of the response header's controls, the initiator
 *  header's only one. The other two flip a transcript-wide setting; this one
 *  folds THIS turn to its `⋯` stub.
 *
 *  **It is the one turn control whose glyph changes with its state.** The pair
 *  beside it keeps a fixed glyph and brightens (see `FullResponseIcon`). This
 *  one is exempt from that brightness rule (`.turn-controls` in
 *  styles/chat/input-messages.css): bright meaning FOLDED would invert what
 *  bright means on its neighbours. So the glyph is its only visible state cue,
 *  and it names the next click, agreeing with the tooltip.
 *
 *  The circle is the same in both forms, so the flip adds one stroke inside it
 *  and the mark never changes size. `__tests__/turn-controls.test.tsx` pins
 *  that. */
export function CollapseTurnIcon({ collapsed = false }: { collapsed?: boolean }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round">
      <circle cx="12" cy="12" r="8.5"/>
      <line x1="8.5" y1="12" x2="15.5" y2="12"/>
      {collapsed && <line x1="12" y1="8.5" x2="12" y2="15.5"/>}
    </svg>
  );
}

/** Step-log glyph: three leader-dot lines, the shape of the tool-by-tool log
 *  the steps toggle reveals. Deliberately not `TodoListIcon`'s ticked box,
 *  which already means the todo list; the two would otherwise sit within a
 *  few rem of each other on the same turn. */
export function StepLogIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="4.5" cy="7" r="1.15" fill="currentColor" stroke="none"/>
      <line x1="8.5" y1="7" x2="20" y2="7"/>
      <circle cx="4.5" cy="12" r="1.15" fill="currentColor" stroke="none"/>
      <line x1="8.5" y1="12" x2="20" y2="12"/>
      <circle cx="4.5" cy="17" r="1.15" fill="currentColor" stroke="none"/>
      <line x1="8.5" y1="17" x2="20" y2="17"/>
    </svg>
  );
}

export function FileIcon({ size = '1rem' }: { size?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M13 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V9z"/>
      <polyline points="13 2 13 9 20 9"/>
    </svg>
  );
}

/** Unified-diff glyph: an added line, a context line, a removed line. Toggle
 *  counterpart to FileIcon.
 *
 *  THREE rows, not two. The `+` and `-` pair sat across the middle third of the
 *  box and read as squat. The report on the composer's Diff button named
 *  exactly that. A hunk is the smallest shape saying "diff" rather than "two
 *  lines", and it fills the box.
 *
 *  The middle row carries no marker on purpose. An unmarked gutter IS what a
 *  context line looks like, and marking all three would say every line
 *  changed. */
export function DiffIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="5" y1="3" x2="5" y2="7" /><line x1="3" y1="5" x2="7" y2="5" />
      <line x1="10" y1="5" x2="21" y2="5" />
      <line x1="10" y1="12" x2="21" y2="12" />
      <line x1="3" y1="19" x2="7" y2="19" />
      <line x1="10" y1="19" x2="21" y2="19" />
    </svg>
  );
}

/** Two columns side by side: the side-by-side diff glyph. Toggle counterpart to
 *  DiffIcon, whose single column is the unified rendering. */
export function SideBySideColumnsIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="3" y="4" width="7.5" height="16" rx="1" />
      <rect x="13.5" y="4" width="7.5" height="16" rx="1" />
    </svg>
  );
}

export function BackIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="10 3 5 8 10 13" /></svg>
  );
}

export function ForwardIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"><polyline points="6 3 11 8 6 13" /></svg>
  );
}

export function SearchIcon({ className }: { className?: string } = {}) {
  return (
    <svg class={className} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="7" cy="7" r="4.5" />
      <path d="M10.5 10.5L14 14" />
    </svg>
  );
}

export function ComposeIcon() {
  return (
    <svg viewBox="-1 -1 18 18" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <path d="M8 2H3a2 2 0 0 0-2 2v8a2 2 0 0 0 2 2h8a2 2 0 0 0 2-2V9.5" />
      <path d="M12.5 1.5l-6 6V10h2.5l6-6-2.5-2.5z" />
    </svg>
  );
}

export function DraftsIcon({ size }: { size?: string } = {}) {
  return (
    <svg {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <rect x="4" y="2" width="16" height="20" rx="2" />
      <line x1="8" y1="9" x2="16" y2="9" />
      <line x1="8" y1="13" x2="16" y2="13" />
      <line x1="8" y1="17" x2="16" y2="17" />
    </svg>
  );
}

export function AttentionIcon({ size }: { size?: string } = {}) {
  return (
    <svg {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="10" />
      <line x1="12" y1="8" x2="12" y2="12" />
      <line x1="12" y1="16" x2="12.01" y2="16" />
    </svg>
  );
}

// The thread-drawer toggle on desktop: a window with its left column split off,
// the glyph desktop apps use for a sidebar. Never a stack of lines: the Canvas
// hamburger is already that, and look-alikes in one bar hide the one you are
// looking for. Desktop only, because on a phone the toggle opens a whole pane
// rather than a sidebar (see `ThreadListIcon`).
//
// Frame and divider stay one path so their crossing is stroked once. Two
// elements paint it twice, and a translucent colour shows each overlap as a dot.
export function SidebarIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M5 4h14a2 2 0 0 1 2 2v12a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V6a2 2 0 0 1 2-2zM9 4v16" />
    </svg>
  );
}

// The thread-drawer toggle on a phone: a bulleted list, since it takes the user
// to the threads pane, which is a list of threads. Filter holds the same corner
// of that pane, which is why Filter draws a funnel and never lines.
export function ThreadListIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" stroke="none">
      <circle cx="4" cy="6" r="1.5" />
      <line x1="9" y1="6" x2="21" y2="6" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" />
      <circle cx="4" cy="12" r="1.5" />
      <line x1="9" y1="12" x2="21" y2="12" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" />
      <circle cx="4" cy="18" r="1.5" />
      <line x1="9" y1="18" x2="21" y2="18" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" />
    </svg>
  );
}

export function MenuIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="4" y1="6" x2="20" y2="6" />
      <line x1="4" y1="12" x2="20" y2="12" />
      <line x1="4" y1="18" x2="20" y2="18" />
    </svg>
  );
}

// A funnel shape, never a stack of lines. On a phone, Filter takes the corner
// where the pane beside it has the list-shaped thread-drawer toggle. Lines there
// read as the way back.
const FUNNEL_PATH = 'M2 2.5h12L9.5 8.25V14l-3-1.5V8.25z';

// Unified thread-drawer Filter control. One button toggles the merged Status +
// Thread type panel (see ThreadFilterPanel); also the "All statuses" row icon
// inside that panel.
export function FilterIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <path d={FUNNEL_PATH} />
    </svg>
  );
}

// FUNNEL_PATH as it looks stroked at 1.5 with round joins, traced as one outline.
// The filled funnel paints this alone: a translucent header colour paints a
// stroke laid over a fill twice, and the rim comes out brighter than the body.
const FUNNEL_SILHOUETTE_PATH =
  'M1.409 2.962A.75.75 0 0 1 2 1.75H14a.75.75 0 0 1 .591 1.212L10.25 8.509V14a.75.75 0 0 1-1.085.671l-3-1.5A.75.75 0 0 1 5.75 12.5V8.509z';

// The Filter button and the panel's "All statuses" row while a thread-type
// selection narrows the `all` view: the same funnel, filled. Outline for off and filled for on is the iOS convention.
export function FilteredIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="currentColor" stroke="none">
      <path d={FUNNEL_SILHOUETTE_PATH} />
    </svg>
  );
}

export function PopOutIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <polyline points="15 3 21 3 21 9" />
      <line x1="10" y1="14" x2="21" y2="3" />
    </svg>
  );
}

/** The mirror of {@link PopOutIcon}: the same frame, with the arrow landing
 *  INSIDE it rather than leaving it. "Switch this window", where the pop-out
 *  says "open another one". */
export function PopInIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M18 13v6a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h6" />
      <polyline points="12 6 12 12 18 12" />
      <line x1="21" y1="3" x2="12" y2="12" />
    </svg>
  );
}

export function FullscreenIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="15 3 21 3 21 9" />
      <polyline points="9 21 3 21 3 15" />
      <line x1="21" y1="3" x2="14" y2="10" />
      <line x1="3" y1="21" x2="10" y2="14" />
    </svg>
  );
}

export function ExitFullscreenIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="4 14 10 14 10 20" />
      <polyline points="20 10 14 10 14 4" />
      <line x1="14" y1="10" x2="21" y2="3" />
      <line x1="3" y1="21" x2="10" y2="14" />
    </svg>
  );
}

export function GlobeIcon({ size = '0.875rem' }: { size?: string }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <circle cx="12" cy="12" r="10" /><path d="M2 12h20" /><path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
    </svg>
  );
}

export function CodeIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="18 16 22 12 18 8" /><polyline points="6 8 2 12 6 16" /><line x1="14.5" y1="4" x2="9.5" y2="20" />
    </svg>
  );
}

/** Soft wrap: three lines of text, the second one turning back on itself. The
 *  return arrow is what says "wrap" rather than "a list", so it keeps its own
 *  head even at the smallest icon size. */
export function WrapTextIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="3" y1="6" x2="21" y2="6" />
      <path d="M3 12h13a3.5 3.5 0 0 1 0 7h-3" />
      <polyline points="15 16 12 19 15 22" />
      <line x1="3" y1="19" x2="8" y2="19" />
    </svg>
  );
}

export function CodexIcon() {
  return (
    <svg class="codex-icon" viewBox="0 0 24 24" fill="none" stroke="var(--accent-light)" stroke-width="2.25" stroke-linecap="round" stroke-linejoin="round">
      <g transform="translate(-1.2 -1.2) scale(1.1)">
        <path d="M9.6 5.5c1.4-2 4.7-1.9 6 .2.9.2 1.7.8 2.2 1.7.5.8.6 1.7.4 2.6 2 .5 3.3 2.1 3.3 4.1 0 1.3-.6 2.5-1.6 3.3.1 2.3-1.8 4.1-4.2 4.1-1 0-1.9-.3-2.6-.9-.8.8-1.9 1.2-3.1 1.2-2 0-3.6-1.2-4.2-2.8-2-.3-3.5-1.9-3.5-3.8 0-1.7 1-3.2 2.6-3.8-.3-2.4 1.6-4.5 4-4.5.3 0 .5 0 .7.1Z" />
        <path d="M8.4 10.2 10.4 12l-2 1.8" />
        <path d="M13.5 14h2.4" />
      </g>
    </svg>
  );
}

export function EyeIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M1 12s4-8 11-8 11 8 11 8-4 8-11 8-11-8-11-8z" /><circle cx="12" cy="12" r="3" />
    </svg>
  );
}

export function CheckIcon({ className, size }: { className?: string; size?: string } = {}) {
  return (
    <svg class={className} width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="20 6 9 17 4 12" />
    </svg>
  );
}

/** Ink for each finished step outcome, drawn to fill a 12-unit box so a 1cap
 *  svg spans the text's cap band exactly (see `.step-icon svg`, steps.css). */
const STEP_OUTCOME_INK: Record<Exclude<StepOutcome, 'pending'>, preact.JSX.Element> = {
  success: <path d="M1 6.5 4.5 10.5 11 1.5" />,
  error: (
    <>
      <circle cx="6" cy="6" r="5.25" />
      <path d="M6 3.25v3.25" />
      <circle cx="6" cy="8.75" r="0.5" fill="currentColor" />
    </>
  ),
  unfinished: (
    <>
      <circle cx="6" cy="6" r="5.25" />
      <path d="M2.3 9.7 9.7 2.3" />
    </>
  ),
  blocked: <path d="M4 1v10M8 1v10" />,
  denied: <path d="M2 2l8 8M10 2l-8 8" />,
};

/** The leading mark on an inline step row. A running step has none: its
 *  shimmering description is the live affordance. */
export function StepOutcomeIcon({ outcome }: { outcome: StepOutcome }) {
  if (outcome === 'pending') return null;
  return (
    <svg class={`step-outcome-icon-${outcome}`} viewBox="0 0 12 12" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      {STEP_OUTCOME_INK[outcome]}
    </svg>
  );
}

export function EditIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M11 4H4a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h14a2 2 0 0 0 2-2v-7" />
      <path d="M18.5 2.5a2.121 2.121 0 0 1 3 3L12 15l-4 1 1-4 9.5-9.5z" />
    </svg>
  );
}

export function EyeOffIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M9.88 9.88a3 3 0 0 0 4.24 4.24" />
      <path d="M10.73 5.08A10.43 10.43 0 0 1 12 5c7 0 11 7 11 7a13.16 13.16 0 0 1-1.67 2.68" />
      <path d="M6.61 6.61A13.526 13.526 0 0 0 1 12s4 7 11 7a9.74 9.74 0 0 0 5.39-1.61" />
      <line x1="2" y1="2" x2="22" y2="22" />
    </svg>
  );
}

// Drawer "Review" view — an open eye (changes ready to look over / apply).
export function ReviewIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M2 12s3.5-7 10-7 10 7 10 7-3.5 7-10 7-10-7-10-7z" />
      <circle cx="12" cy="12" r="3" />
    </svg>
  );
}

// Drawer "Running" indicator — a STATIC ring spinner, the same visual family as
// the animated `.mini-spinner` on running thread rows (modal-overlay.css), so the
// app shows one spinner shape. It stays static everywhere it labels the "running"
// category (the threads-header Filter button, the Filter dropdown's Running row,
// the RUNNING section header); only the per-thread spinners actually animate —
// motion only where work is in flight.
export function RunningIcon({ size }: { size?: string } = {}) {
  return (
    <svg {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2.5" stroke-linecap="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9" pathLength="100" stroke-dasharray="75 25" transform="rotate(-90 12 12)" />
    </svg>
  );
}

// Drawer "Current" section — an inbox tray (the live working set).
export function InboxIcon({ size }: { size?: string } = {}) {
  return (
    <svg {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M22 12h-6l-2 3h-4l-2-3H2" />
      <path d="M5.45 5.11 2 12v6a2 2 0 0 0 2 2h16a2 2 0 0 0 2-2v-6l-3.45-6.89A2 2 0 0 0 16.76 4H7.24a2 2 0 0 0-1.79 1.11z" />
    </svg>
  );
}

// Drawer "Archive" section — a lidded storage box.
export function ArchiveIcon({ size }: { size?: string } = {}) {
  return (
    <svg {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <rect x="3" y="4" width="18" height="4" rx="1" />
      <path d="M5 8v11a1 1 0 0 0 1 1h12a1 1 0 0 0 1-1V8" />
      <line x1="10" y1="12" x2="14" y2="12" />
    </svg>
  );
}

// Filter panel "Status" heading: a dot in a ring, like a status light.
export function StatusIcon({ size }: { size?: string } = {}) {
  return (
    <svg {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <circle cx="12" cy="12" r="9" />
      <circle cx="12" cy="12" r="3" fill="currentColor" />
    </svg>
  );
}

// Filter panel "By thread types" heading: stacked layers, one per type.
export function ThreadTypesIcon({ size }: { size?: string } = {}) {
  return (
    <svg {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <polygon points="12 2 2 7 12 12 22 7 12 2" />
      <polyline points="2 17 12 22 22 17" />
      <polyline points="2 12 12 17 22 12" />
    </svg>
  );
}

// Compose Send button — an up-arrow inside the round send/cancel morph button.
export function SendArrowIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="3.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <line x1="12" y1="21" x2="12" y2="3" />
      <polyline points="4 11 12 3 20 11" />
    </svg>
  );
}

// Compose Cancel/Stop button — a filled stop-square inside the round morph button.
export function StopIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" stroke="none" aria-hidden="true">
      <rect x="6" y="6" width="12" height="12" rx="2" />
    </svg>
  );
}

// The standard pause glyph: two filled bars. Paints the `paused` thread status
// (a turn an engine restart interrupted). Same filled family as StopIcon, and
// deliberately a recognizable transport glyph rather than another colored dot,
// because "paused" is the one status a universal symbol already says outright.
//
// Unlike its neighbours in this file, the viewBox HUGS the bars rather than
// centring them in a square box, and its consumer sizes it to that same 12:16
// aspect. Every other occupant of the thread status slot (a 0.4rem dot, the
// 0.7rem question badge, the spinner) is a shape that fills its box, and the
// slot is a flex row that left-aligns them, so ink floating in the middle of a
// square icon box would leave a paused row's glyph visibly indented against
// every neighbouring row's dot.
export function PauseIcon() {
  return (
    <svg viewBox="0 0 12 16" fill="currentColor" stroke="none" aria-hidden="true">
      <rect x="0" y="0" width="4.5" height="16" rx="1" />
      <rect x="7.5" y="0" width="4.5" height="16" rx="1" />
    </svg>
  );
}

// Power: the restart control in the Lucidos menu. A power symbol rather than
// another circular arrow, because the row it sits under is Refresh and the two
// must not read as the same action twice: one reloads the client, this one
// stops and starts the workspace's engine.
export function PowerIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M12 3v9" />
      <path d="M6.4 6.4a8 8 0 1 0 11.2 0" />
    </svg>
  );
}

/** The *trigger* actor chip's glyph: a trigger fired and this turn is the
 *  result. Worn by the `TriggerStarted` initiator panel.
 *
 *  Deliberately NOT `EventWaitClockIcon`, the clock a few lines up, and the two
 *  can appear within a screen of each other in one transcript. That one marks an
 *  *event wait*: something that has NOT happened, which will wake the turn when
 *  it does. This marks the opposite, something that already fired. A clock here
 *  would also quietly claim every trigger is scheduled, which is wrong: an event
 *  trigger has no clock in it at all.
 *
 *  FILLED, against this file's outlined default, and the reason is the shape
 *  rather than a preference. A bolt's waist is where its two halves cross, and
 *  an outline puts two strokes through that crossing: at the `--icon-size-sm`
 *  the chip renders (0.875rem, ~14px on a plain desktop root, so 24 units map to
 *  ~0.58px each) the 2-unit stroke is ~1.2px and the daylight left in the waist
 *  is under a device pixel. A solid body has no interior to lose, so it
 *  degrades to a smaller bolt rather than to a blob. The cost is ink: this is
 *  heavier than the outlined marks around it. That is tolerable, because the
 *  chip holds exactly one glyph and nothing competes with it. */
export function TriggerFiredIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" stroke="currentColor" stroke-width="1.6" stroke-linejoin="round" aria-hidden="true">
      <polygon points="13 2 4 14 11 14 10 22 19 10 12 10 13 2" />
    </svg>
  );
}

/** The *You* actor chip's glyph: this turn was started from one of your
 *  devices. The one origin the chip is allowed to call "You" (see
 *  `actorInitiator`), so it must not resemble `ApiPlugIcon` below, which is
 *  precisely the origin that must never be mistaken for you.
 *
 *  FILLED, for the reason `TriggerFiredIcon` sets out at length and for the same
 *  measured cause: the outlined form's daylight is the gap between the head and
 *  the shoulders, ~2 units, and at chip size that closes into a smudge with a
 *  notch. Solid keeps head and shoulders legible as two masses at any size.
 *  The head sits clear of the cap rather than touching it, so the pair still
 *  reads as a figure and not as a single lump. */
export function PersonIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="currentColor" stroke="none" aria-hidden="true">
      <circle cx="12" cy="7.75" r="4" />
      <path d="M12 13.5c-4.4 0-8 2.9-8 6.5v1h16v-1c0-3.6-3.6-6.5-8-6.5z" />
    </svg>
  );
}

/** The *API caller* actor chip's glyph: an external HTTP caller that did not
 *  self-identify (no device id, no agent-origin token, no known workspace).
 *
 *  A plug, keeping the metaphor `API_CALLER_LABEL`'s own definition reaches for:
 *  an external integration plugging into the API. Outlined rather than filled,
 *  unlike its two neighbours above, and that difference is load-bearing rather
 *  than incidental. The chip exists so an anonymous mutating POST can never be
 *  rendered as "You", so this glyph's whole job is to not be mistaken for
 *  `PersonIcon`: two open prongs over a hollow body against one solid mass is a
 *  difference in weight as well as in shape, legible before either outline is.
 *  The prongs are 6 units apart, three times the daylight a stroke needs at chip
 *  size, so what survives shrinking is exactly the part that distinguishes it. */
export function ApiPlugIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M9 2v6" />
      <path d="M15 2v6" />
      <path d="M6 8.5h12V12a6 6 0 0 1-12 0z" />
      <path d="M12 18v4" />
    </svg>
  );
}

/** A directory row in the directory picker. Renders at 1.25rem rather than the
 *  chip's 0.875rem, which is what affords a single closed outline here where the
 *  actor chips needed solids. */
export function FolderIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M3 7a2 2 0 0 1 2-2h4l2.25 2.75H19a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
    </svg>
  );
}

/** The `..` row in the directory picker: go up to the parent directory.
 *
 *  A folder with an arrow out of it, not a second folder shape. The row is a
 *  BUTTON that moves you, where every row under it names a place, so the arrow
 *  is carrying the only thing that distinguishes them. It affords three marks
 *  only because this slot is 1.25rem; at the chip size used elsewhere in this
 *  file an arrow inside a folder smudges into one blob, so do not reuse this one
 *  in a chip. */
export function FolderUpIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M3 7a2 2 0 0 1 2-2h4l2.25 2.75H19a2 2 0 0 1 2 2V18a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z" />
      <polyline points="9.5 15 12 12.5 14.5 15" />
      <line x1="12" y1="12.5" x2="12" y2="17.5" />
    </svg>
  );
}

/** THE apps glyph. A package: a box with its lid seam and front edge.
 *
 *  Single definition on purpose, and the single one there is. Three surfaces
 *  mark an app and all three read it from here: Search Everywhere's result rows
 *  and the content-pane back/forward history menu (both via `CategoryIcon`'s
 *  `apps` case, which renders this rather than drawing its own), and the message
 *  route panel's fallback for an app whose manifest declares no icon. A concept
 *  with two glyphs diverges on the next tweak, which is what this replaced: the
 *  category set carried a 2x2 rounded tile grid, one spark short of the Lucidos
 *  mark that appears on every Lucidos Agent chip in the same transcript.
 *
 *  Authored in a 16-unit box at stroke-width 1.5, NOT this file's 24-unit
 *  stroke-2 default, and it carries its own `width`/`height` where most icons
 *  here take theirs from a class. Both come from the category family, whose
 *  glyphs all take exactly those numbers from one shared props spread in
 *  `CategoryIcon`. Matching the stroke keeps it from sitting heavier than the
 *  file, thread and trigger glyphs beside it in a row.
 *
 *  The inline SIZE is load-bearing rather than tidy, and dropping it is a real
 *  bug rather than a style slip. `.search-everywhere-result-icon`, the slot on
 *  every Search Everywhere result row, sizes its own box and has NO RULE for
 *  the `<svg>` inside it. That surface has always sized its glyphs purely from
 *  the attributes that spread put on each `<svg>`. So an unsized `AppsIcon` there falls back to the default replaced
 *  element box and paints an app hit's mark at ~300px. Its other two consumers
 *  hide the fault, because both do size the slot in CSS
 *  (`.nav-history-icon svg`, `.message-route-panel .route-app-icon svg`), and a
 *  CSS rule beats a presentation attribute, so those keep overriding this
 *  default exactly as before. */
export function AppsIcon({ size = '1rem' }: { size?: string } = {}) {
  return (
    <svg width={size} height={size} viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">
      <path d="M2 5 8 2l6 3v6l-6 3-6-3z" />
      <path d="m2 5 6 3 6-3" />
      <path d="M8 8v6" />
    </svg>
  );
}

// The Lucidos mark: three rounded squares and a four-point spark, the same
// geometry as `public/favicon.svg` and the installed PWA icon. Kept as paths
// with no tile behind them. So one copy of the artwork serves both uses: on
// the brand gradient, or painted flat in a single colour.
//
// `fill: currentColor` rather than the favicon's hardcoded white, because a
// flat variant is exactly the same shape in a different colour.
export function LucidosMarkIcon() {
  return (
    <svg class="lucidos-mark-icon" viewBox="0 0 100 100" fill="currentColor" stroke="none" aria-hidden="true">
      <g transform="translate(13 13) scale(0.74)">
        <rect x="17" y="17" width="29" height="29" rx="7" />
        <rect x="17" y="54" width="29" height="29" rx="7" />
        <rect x="54" y="54" width="29" height="29" rx="7" />
        <path d="M68.5 12 C71 25 74 28.5 87 31 C74 33.5 71 37 68.5 50 C66 37 63 33.5 50 31 C63 28.5 66 25 68.5 12 Z" />
      </g>
    </svg>
  );
}
