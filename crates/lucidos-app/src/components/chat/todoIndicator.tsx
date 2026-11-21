import { signal } from '@preact/signals';
import { focusedThreadId, threadMap } from '../../store/store';
import type { TodoItem } from '../../store/thread-events';
import type { HeaderActionSpec } from '../layout/headerActions';
import { TodoListIcon } from '../shared/icons';
import { lazyComponent, whenLoaded, type PendingOpen } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

/** What the panel is positioned against, or null while it is closed.
 *
 *  Module-level, and one signal for both ways in. The indicator folds into the
 *  composer row's ⋯ menu on a short row, so the button that opens the panel may
 *  not exist. A press on the row button passes itself. A press on the menu row
 *  passes the ⋯ trigger, the box the reader pressed. */
export const todoPanelAnchor = signal<HTMLElement | null>(null);

/** Close the panel. Called by the composer when a fold moves controls around,
 *  since an anchor can leave the DOM under an open panel. */
export function closeTodoPanel(): void {
  pendingOpen?.cancel();
  todoPanelAnchor.value = null;
}

/** The two values `data-state` can take. A union rather than `string`, so a
 *  third one cannot be stamped without a stylesheet rule to paint it. */
type TodoIndicatorState = 'in-progress' | 'idle';

/** Everything the indicator says about a list, in one place.
 *
 *  `null` when there is nothing to report, which is what hides the control.
 *  *Todo notes* can outlive the items: under ADR 0085's context mode an agent
 *  that finished its plan still writes `todos: []` with a pointer worth
 *  keeping. Hiding the indicator there would make the block the whole mode
 *  rests on the one thing the user cannot see.
 *
 *  TWO states, stamped as `data-state` and painted in todo-list.css over one
 *  shared glyph: `in-progress` takes the accent, `idle` takes the row's
 *  ordinary gray. That is the composer row's whole language, the one the follow
 *  toggle and the *waiting indicator* beside it already speak: accent means
 *  something is live right now. A parked or abandoned item has no paint of its
 *  own, since the waiting indicator already says the first.
 *
 *  The WORDS keep the fuller picture, and there waiting outranks abandoned
 *  because it is the live fact: a list carrying both has parked items that are
 *  still going somewhere. */
export function todoIndicatorSummary(
  items: TodoItem[] | null,
  notes?: string | null,
): {
  state: TodoIndicatorState;
  tooltip: string;
  ariaLabel: string;
  menuLabel: string;
} | null {
  if (items === null) return null;
  if (items.length === 0) {
    if (!notes) return null;
    return {
      state: 'idle',
      tooltip: 'Notes kept',
      ariaLabel: 'Todo list: no items, notes kept. Click to expand.',
      menuLabel: 'Todo list: notes kept',
    };
  }
  const total = items.length;
  const completed = items.filter((i) => i.status === 'completed').length;
  const waiting = items.filter((i) => i.status === 'waiting').length;
  const abandoned = items.filter((i) => i.status === 'abandoned').length;
  const inProgress = items.find((i) => i.status === 'in_progress');
  const state = inProgress ? 'in-progress' : 'idle';
  const counted = waiting > 0
    ? `${completed} of ${total} done, ${waiting} waiting`
    : abandoned > 0
      ? `${completed} of ${total} done, ${abandoned} abandoned`
      : `${completed} of ${total} done`;
  const tooltip = inProgress ? inProgress.active_form : counted;
  // The aria-label names the state, it does not just count. Colour is now the
  // ONLY visual channel carrying it, and a screen reader cannot read colour.
  // Forced-colors mode overwrites it outright: there --accent collapses to the
  // system foreground, so in-progress and idle paint identically. The tooltip
  // cannot stand in either, being desktop-hover only.
  const said = inProgress ? `${inProgress.active_form}. ${completed} of ${total} done` : counted;
  return {
    state,
    tooltip,
    ariaLabel: `Todo list: ${said}. Click to expand.`,
    // The menu row's words. A folded indicator has no paint at all, so this is
    // the whole of what it reports.
    menuLabel: `Todo list: ${said}`,
  };
}

export function todoListIndicatorBody({
  items,
  notes,
  onClick,
  attrs,
}: {
  items: TodoItem[] | null;
  notes?: string | null;
  onClick: (e: MouseEvent) => void;
  /** The row attributes the composer's fold cluster stamps on every member. */
  attrs?: Record<string, string>;
}) {
  const summary = todoIndicatorSummary(items, notes);
  if (!summary) return null;
  return (
    <button
      {...attrs}
      type="button"
      class="icon-btn header-icon"
      data-role="todo-indicator"
      data-state={summary.state}
      data-tooltip={summary.tooltip}
      aria-label={summary.ariaLabel}
      onClick={onClick}
      data-row-item
    >
      <TodoListIcon />
    </button>
  );
}

/** The panel's chunk, fetched once the boot splash lifts (ADR 0288). */
const TodoPanelHost = lazyComponent(() => import('./TodoListPanel').then((m) => m.TodoPanelHost));
prefetchWhenIdle(TodoPanelHost);

/** Where the composer mounts the panel. Its own component, so opening and
 *  closing re-renders this slot rather than the whole composer. The panel's
 *  `<Overlay>` renders nothing while shut, so mounting it only while open
 *  changes nothing on screen. */
export function TodoPanelSlot() {
  return todoPanelAnchor.value ? <TodoPanelHost /> : null;
}

/** The last open that waited on the panel's chunk. */
let pendingOpen: PendingOpen | null = null;

/** Open the panel against `anchor` once its chunk is in memory, so it never
 *  opens empty. A second press while that is pending takes the open back. An
 *  anchor a fold removed meanwhile is not opened against. */
function openTodoPanel(anchor: HTMLElement): void {
  if (pendingOpen?.pending) {
    closeTodoPanel();
    return;
  }
  pendingOpen = whenLoaded(TodoPanelHost, () => {
    if (anchor.isConnected) todoPanelAnchor.value = anchor;
  }, anchor);
}

/** The todo indicator, as one of the composer row's foldable actions.
 *
 *  `null` when the chat agent has written nothing to show. Reads
 *  `meta.latestTodoList` (projected in `handleEvent`) so the render path is
 *  O(1), with no walk of the events Map per `threadMap` flush.
 *
 *  Folded, the row button is gone and the menu row is the whole control. It
 *  carries the same words the button's accessible name does, which is the only
 *  channel a menu row has: colour and a pulse do not survive the fold. */
export function todoIndicatorAction(): HeaderActionSpec | null {
  const id = focusedThreadId.value;
  const meta = id ? threadMap.value.get(id)?.meta : undefined;
  const items = meta?.latestTodoList ?? null;
  const notes = meta?.latestTodoNotes ?? null;
  const summary = todoIndicatorSummary(items, notes);
  if (!summary) return null;
  return {
    key: 'todo-indicator',
    dataRole: 'todo-indicator',
    label: summary.menuLabel,
    tooltip: summary.tooltip,
    icon: () => <TodoListIcon />,
    render: (attrs) => todoListIndicatorBody({
      items,
      notes,
      attrs,
      // Re-pressing the button closes, which is the toggle the anchor exemption
      // in <Overlay> leaves to the control's own handler.
      onClick: (e) => {
        if (todoPanelAnchor.value) closeTodoPanel();
        else openTodoPanel(e.currentTarget as HTMLElement);
      },
    }),
    onMenuClick: (anchor) => { if (anchor) openTodoPanel(anchor); else closeTodoPanel(); },
  };
}
