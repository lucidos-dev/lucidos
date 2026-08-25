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

export function PinIcon({ filled = false, size }: { filled?: boolean; size?: string }) {
  return (
    <svg {...(size ? { width: size, height: size } : {})} viewBox="0 0 24 24" fill={filled ? 'currentColor' : 'none'} stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
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

/** A horseshoe MAGNET, poles UP: the *standing follow*'s toggle, in the
 *  prompt area. It says stick to the live edge, where the chevron beside it
 *  says go there once.
 *
 *  It was an arrow coming down onto a line, and that glyph is `DownloadIcon`.
 *  The two are not far apart: Download wears it on the thread header's
 *  "Download thread" row, one row above this composer. A labelled menu item can
 *  carry a shared mark. An icon-only toggle cannot, so the toggle is the one
 *  that moved.
 *
 *  Deliberately NOT `ChevronDownIcon`, which the scroll button keeps. Those two
 *  stopped being one button precisely because they cannot be: the chevron
 *  NAVIGATES to the bottom, this one STAYS there.
 *
 *  No inline size, unlike the two chevrons above. This renders inside
 *  `.icon-btn`, whose class sizes the svg and whose rule bans an inline size
 *  for that reason (see `FullResponseIcon`). Poles UP is a legibility call.
 *  Inverted, the bands land beside the leg ends and smudge into one foot,
 *  the failure `CollapseTurnIcon` records. Up, they sit against open space
 *  and stay separate down to 14px. */
export function FollowLiveEdgeIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <path d="M4 4v8a8 8 0 0 0 16 0V4h-5v8a3 3 0 0 1-6 0V4z"/>
      <path d="M4 8h5"/>
      <path d="M15 8h5"/>
    </svg>
  );
}

/** One chevron: there is more of this response than you are being shown.
 *  Worn by the response header's full-response toggle.
 *
 *  It does NOT change with the toggle's state, which took two goes to get
 *  right. It started as an unfold/fold pair, on the reasoning that the text
 *  link it replaced said which way the next click went ("More" / "Less") and a
 *  fixed glyph cannot. But the two forms have the same box and very different
 *  ink (points at the extremes against bars at the extremes), so the mark
 *  visibly changed size on every click while the body under it was also
 *  moving: reported as the layout dancing. Brightness already answers "is it
 *  on" (see `.turn-controls` in styles/chat/input-messages.css), which is
 *  exactly how the neighbouring steps control has always worked.
 *
 *  Kept here rather than reusing `ChevronDownIcon`, which hardcodes a 1.25rem
 *  inline width/height for the scroll-to-bottom button. Inside `.icon-btn` the
 *  class has to override that, and the CSS rule bans an inline size for
 *  exactly that reason. Drawn a touch wider and deeper than that one, which is
 *  what keeps it legible at the 0.875rem this renders at. */
export function FullResponseIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points="5 9 12 16 19 9"/>
    </svg>
  );
}

/** Two arrowheads, pointing at each other to fold this turn away and apart to
 *  unfold it. Worn by both turn headers: the third of the response header's
 *  controls, the initiator header's only one. The odd one out of that run: the
 *  other two flip a transcript-wide setting, this one folds THIS turn to its
 *  `⋯` stub.
 *
 *  It was drawn with a full-width line between the two, on the reasoning that
 *  the line is what the arrows are being squashed onto. Three horizontal marks
 *  do not survive the size this renders at. `--icon-size-sm` is 0.875rem, so
 *  the 24-unit box is 14px on a plain desktop root: the tips sat 3.5 units off
 *  the line, the 2-unit stroke ate 2 of those, and the ~0.9px left over is
 *  under a device pixel. The three marks smudged into one blob, while both
 *  neighbours are single open marks with air around them and read fine at the
 *  same size. Dropping the line leaves the two arrowheads a clear channel
 *  between them, and the convergence carries the meaning on its own.
 *
 *  Deliberately shallower and narrower than `FullResponseIcon`, which is one
 *  wide chevron two buttons to the left in the same group: these read as
 *  arrowheads pointing at each other, not as a chevron and its mirror.
 *
 *  **This is the one control whose glyph moves with its state, and that is a
 *  deliberate exception to the rule `FullResponseIcon` above records.** Not a
 *  loophole in it: the banned `UnfoldIcon` was built exactly like this, two
 *  arrowheads each reflected about its own midline, and its two forms shared a
 *  span (x 5 to 19, y 2 to 22), a summed segment length and a stroke count just
 *  as these do. So "same box, same ink" is NOT what distinguishes this case, and
 *  an argument from geometry here would be false. What made that mark look like
 *  it changed size is the property both pairs have: converging puts the two wide
 *  endpoints at the extremes of the box, diverging puts the two apexes there.
 *
 *  The exception is bought with a different currency. That flip was pure cost,
 *  because `aria-pressed` plus the brightness rule already answered "is it on",
 *  so the movement bought nothing. This control is exempt from that brightness
 *  rule (see `.turn-controls` in styles/chat/input-messages.css), because bright
 *  meaning FOLDED inverts what bright means on the two controls beside it. So
 *  direction is not one state cue among two here, it is the only one there is.
 *  It also points the way the next click goes, agreeing with the tooltip
 *  ("Collapse this turn" / "Expand this turn").
 *
 *  What IS carried over is the other half of that commit's complaint, which was
 *  that the mark was simply too big: 22 of 24 units of ink against the log
 *  glyph's 12, towering over the label and its neighbour. This one is 14 with
 *  its round caps, two units over the log glyph and no longer standing out of
 *  the row. It was 18 first, on the reasoning that a 6-unit gap between the
 *  apexes is what stops the two arrowheads fusing at 14px. The gap IS the thing
 *  that has to survive, but 6 was overpaying for it, and the reported complaint
 *  was precisely the air in the middle: 4 units leaves 2 of daylight after the
 *  stroke, ~1.2px at 14px and a clear channel on any 2x display.
 *
 *  Note what the smudge that started this is NOT evidence for. It was the same
 *  SHAPE as this: those arrowheads pinched at one x too (their tips 3.5 units
 *  off the line, opening to 7.5 at the wing tips), so "point pinch, not a
 *  uniform channel" distinguishes nothing and would be a false argument here.
 *  Three things separate them, and all three are quantities. Its 3.5 left 1.5
 *  of daylight, ~0.9px, UNDER a device pixel where 2 units clears one. There
 *  were two such pinches, because there were three marks. And it opened to 5.5
 *  units of daylight at its widest against this one's 10, since the pair now
 *  spans the full 12 units where those arrowheads spanned 8. The waist runs
 *  both ways: converging it is 4 where the apexes meet in the middle and 12 out
 *  at the wing tips, diverging it is the mirror. The arrowheads lost a unit of
 *  depth each in the same pass (rise 4 over run 6, from 5), since the ask was
 *  total height and halving the gap alone would have left the mark tall and the
 *  two chevrons nearly touching.
 *  `__tests__/turn-controls.test.tsx` pins the envelope so it cannot drift
 *  back toward the banned mark's size. */
export function CollapseTurnIcon({ collapsed = false }: { collapsed?: boolean }) {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <polyline points={collapsed ? '6 10 12 6 18 10' : '6 6 12 10 18 6'}/>
      <polyline points={collapsed ? '6 14 12 18 18 14' : '6 18 12 14 18 18'}/>
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
      <line x1="8.5" y1="17" x2="16" y2="17"/>
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

/** Unified-diff glyph: a `+` line over a `-` line. Toggle counterpart to FileIcon. */
export function DiffIcon() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round">
      <line x1="5" y1="6" x2="5" y2="10" /><line x1="3" y1="8" x2="7" y2="8" />
      <line x1="10" y1="8" x2="21" y2="8" />
      <line x1="3" y1="16" x2="7" y2="16" />
      <line x1="10" y1="16" x2="21" y2="16" />
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

export function ThreadsIcon() {
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

// Unified thread-drawer Filter control: the funnel-style stacked lines. One
// button toggles the merged Status + Thread type panel (see ThreadFilterPanel);
// also the "All statuses" row icon inside that panel.
export function FilterIcon() {
  return (
    <svg viewBox="0 0 16 16" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round">
      <line x1="2" y1="4" x2="14" y2="4" />
      <line x1="4" y1="8" x2="12" y2="8" />
      <line x1="6" y1="12" x2="10" y2="12" />
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
 *  is under a device pixel. That is the arithmetic `CollapseTurnIcon` records
 *  for three horizontal marks, reached here by a different route. A solid body
 *  has no interior to lose, so it degrades to a smaller bolt rather than to a
 *  blob. The tradeoff accepted with it is ink: this is heavier than the outlined
 *  marks it sits among, which is tolerable because the chip holds exactly one
 *  glyph and nothing competes with it inside the slot. */
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
 *  file an arrow inside a folder is exactly the smudge `CollapseTurnIcon` bans,
 *  so do not reuse this one in a chip. */
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
 *  every Search Everywhere result row, HAS NO CSS RULE AT ALL: that surface has
 *  always sized its glyphs purely from the attributes that spread put on each
 *  `<svg>`. So an unsized `AppsIcon` there falls back to the default replaced
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
// with no tile behind them, so a caller can put it on the brand gradient
// (BrandMark's default) or paint it flat in a single colour (the thread
// drawer's muted variant) without two copies of the artwork.
//
// `fill: currentColor` rather than the favicon's hardcoded white, because the
// muted variant is exactly the same shape in a different colour.
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
