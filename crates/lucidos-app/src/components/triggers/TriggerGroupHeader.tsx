import { useState } from 'preact/hooks';
import type { TriggerGroup } from '../../store/types';
import { collapsedTriggerGroupIds, toggleTriggerGroupCollapsed } from '../../store/store';
import { deleteTriggerGroup, renameTriggerGroup } from '../../store/actions/triggerGroups';

interface Props {
  group: TriggerGroup;
}

/** Section header for one trigger group in the panel.
 *
 *  - Chevron toggles the per-device collapsed state (localStorage-backed).
 *  - Member-count badge shows how many triggers are assigned.
 *  - Inline rename: the Rename button swaps the name for an edit input.
 *  - Delete: refused when non-empty; the action handler surfaces the toast.
 */
export function TriggerGroupHeader({ group }: Props) {
  const collapsed = collapsedTriggerGroupIds.value.has(group.id);
  const [editing, setEditing] = useState(false);
  const [draft, setDraft] = useState(group.name);

  async function commit() {
    const trimmed = draft.trim();
    if (!trimmed) { setEditing(false); setDraft(group.name); return; }
    if (trimmed === group.name) { setEditing(false); return; }
    const ok = await renameTriggerGroup(group.id, trimmed);
    if (!ok) setDraft(group.name);
    setEditing(false);
  }

  return (
    <div class={`trigger-group-header${collapsed ? ' trigger-group-collapsed' : ''}`}>
      <button
        class="trigger-group-toggle"
        type="button"
        onClick={() => toggleTriggerGroupCollapsed(group.id)}
        aria-expanded={!collapsed}
        data-tooltip={collapsed ? 'Expand' : 'Collapse'}
      >
        <span class="trigger-group-chevron">{collapsed ? '▸' : '▾'}</span>
        {editing ? (
          <input
            class="trigger-group-name-input"
            type="text"
            value={draft}
            autoFocus
            onClick={e => e.stopPropagation()}
            onInput={e => setDraft((e.target as HTMLInputElement).value)}
            onBlur={commit}
            onKeyDown={e => {
              if (e.key === 'Enter') void commit();
              else if (e.key === 'Escape') { setDraft(group.name); setEditing(false); }
            }}
          />
        ) : (
          <span class="trigger-group-name">{group.name}</span>
        )}
        <span class="trigger-group-count">({group.member_count})</span>
      </button>
      <div class="trigger-group-actions">
        <button
          class="action-btn"
          type="button"
          onClick={e => { e.stopPropagation(); setDraft(group.name); setEditing(true); }}
          data-tooltip="Rename group"
        >
          Rename
        </button>
        <button
          class="action-btn action-btn-danger"
          type="button"
          onClick={e => { e.stopPropagation(); void deleteTriggerGroup(group.id, group.name); }}
          data-tooltip={group.member_count > 0 ? 'Move triggers out first' : 'Delete group'}
          disabled={group.member_count > 0}
        >
          Delete
        </button>
      </div>
    </div>
  );
}
