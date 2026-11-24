import { signal } from '@preact/signals';
import { focusedThreadId, threadMap } from '../../store/store';
import type { TodoItem } from '../../store/thread-events';
import type { HeaderActionSpec } from '../layout/headerActions';
import { TodoListIcon } from '../shared/icons';
import { lazyComponent, whenLoaded, type PendingOpen } from '../../utils/lazyComponent';
import { prefetchWhenIdle } from '../../utils/idlePrefetch';

export const todoPanelAnchor = signal<HTMLElement | null>(null);

export function closeTodoPanel(): void {
  pendingOpen?.cancel();
  todoPanelAnchor.value = null;
}

type TodoIndicatorState = 'in-progress' | 'idle';

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
  const said = inProgress ? `${inProgress.active_form}. ${completed} of ${total} done` : counted;
  return {
    state,
    tooltip,
    ariaLabel: `Todo list: ${said}. Click to expand.`,
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

const TodoPanelHost = lazyComponent(() => import('./TodoListPanel').then((m) => m.TodoPanelHost));
prefetchWhenIdle(TodoPanelHost);

export function TodoPanelSlot() {
  return todoPanelAnchor.value ? <TodoPanelHost /> : null;
}

let pendingOpen: PendingOpen | null = null;

function openTodoPanel(anchor: HTMLElement): void {
  if (pendingOpen?.pending) {
    closeTodoPanel();
    return;
  }
  pendingOpen = whenLoaded(TodoPanelHost, () => {
    if (anchor.isConnected) todoPanelAnchor.value = anchor;
  }, anchor);
}

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
      onClick: (e) => {
        if (todoPanelAnchor.value) closeTodoPanel();
        else openTodoPanel(e.currentTarget as HTMLElement);
      },
    }),
    onMenuClick: (anchor) => { if (anchor) openTodoPanel(anchor); else closeTodoPanel(); },
  };
}
