import type { TriggerGroup } from '../../store/types';
import {
  collapsedTriggerSectionIds,
  toggleTriggerSectionCollapsed,
  UNGROUPED_TRIGGER_SECTION_ID,
} from '../../store/store';
import { deleteTriggerGroup, renameTriggerGroup } from '../../store/actions/triggerGroups';
import { useInlineRename } from '../../hooks/useInlineRename';
import { EditIcon, TrashIcon } from '../shared/icons';
import { PROSE_TEXT_ATTRS } from '../../utils/noAutofill';
import { SectionHeader } from '../shared/SectionHeader';

interface Props {
  group: TriggerGroup;
}

export function UngroupedHeader({ count, collapsed }: { count: number; collapsed: boolean }) {
  return (
    <SectionHeader
      className="trigger-group-header trigger-group-header-ungrouped"
      title="Ungrouped"
      count={count}
      collapsed={collapsed}
      onToggle={() => toggleTriggerSectionCollapsed(UNGROUPED_TRIGGER_SECTION_ID)}
    />
  );
}

export function TriggerGroupHeader({ group }: Props) {
  const collapsed = collapsedTriggerSectionIds.value.has(group.id);
  const {
    renaming: editing,
    draft,
    setDraft,
    inputRef,
    open,
    commit,
    cancel,
  } = useInlineRename(group.name, (next) => renameTriggerGroup(group.id, next));

  return (
    <SectionHeader
      className={`trigger-group-header${editing ? ' trigger-group-renaming' : ''}`}
      collapsed={collapsed}
      onToggle={() => toggleTriggerSectionCollapsed(group.id)}
      count={group.member_count}
      title={<>
        <span class="trigger-group-name-slot">
          <span class="trigger-group-name">{group.name}</span>
          <input
            ref={inputRef}
            class="trigger-group-name-input"
            type="text"
            value={draft}
            {...PROSE_TEXT_ATTRS}
            tabIndex={editing ? 0 : -1}
            aria-hidden={!editing}
            onClick={e => e.stopPropagation()}
            onInput={e => setDraft((e.target as HTMLInputElement).value)}
            onBlur={editing ? commit : undefined}
            data-escape-self
            onKeyDown={e => {
              if (e.key === 'Enter') void commit();
              else if (e.key === 'Escape') { e.preventDefault(); cancel(); }
            }}
          />
        </span>
      </>}
      actions={<>
        <button
          class="icon-btn row-icon trigger-group-rename"
          type="button"
          onClick={e => { e.stopPropagation(); open(); }}
          aria-label={`Rename group “${group.name}”`}
          data-tooltip="Rename group"
        >
          <EditIcon />
        </button>
        <button
          class="icon-btn row-icon trigger-group-delete"
          type="button"
          onClick={e => { e.stopPropagation(); void deleteTriggerGroup(group.id, group.name); }}
          aria-label={`Delete group “${group.name}”`}
          data-tooltip={group.member_count > 0 ? 'Move triggers out first' : 'Delete group'}
        >
          <TrashIcon />
        </button>
      </>}
    />
  );
}
