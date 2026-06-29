import { useState, useRef } from 'preact/hooks';
import { triggers, triggerGroups, collapsedTriggerGroupIds, showToast } from '../../store/store';
import { openAddTrigger } from '../../store/actions/triggers';
import { createTriggerGroup } from '../../store/actions/triggerGroups';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { hasNoMoreRuns, loadedOr } from '../../store/types';
import type { TriggerInfo } from '../../store/types';
import { TriggerItem } from './TriggerItem';
import { TriggerGroupHeader } from './TriggerGroupHeader';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeleton } from '../shared/ListSkeleton';
import { LoadingFade } from '../shared/LoadingFade';

const UNGROUPED_KEY = '__ungrouped__';

function sortByCompletion(a: TriggerInfo, b: TriggerInfo): number {
  // Stopped triggers sink to the bottom within their section, preserving the
  // panel's pre-grouping behavior.
  const aNoMore = hasNoMoreRuns(a);
  const bNoMore = hasNoMoreRuns(b);
  if (aNoMore !== bNoMore) return aNoMore ? 1 : -1;
  return 0;
}

export function TriggersView() {
  const triggersLoadable = triggers.value;
  const groupsLoadable = triggerGroups.value;
  const showTriggersLoading = useDelayedLoading(triggersLoadable);
  const [newGroupName, setNewGroupName] = useState<string | null>(null);
  // Enter and blur BOTH call commitNewGroup; committing unmounts the field,
  // which fires a trailing blur that would POST the same name again (409
  // "already exists", a sticky error toast). Guard with a submit-once flag,
  // reset when the field is reopened.
  const creatingGroupRef = useRef(false);

  async function commitNewGroup() {
    if (creatingGroupRef.current) return;
    const trimmed = (newGroupName ?? '').trim();
    if (!trimmed) { setNewGroupName(null); return; }
    creatingGroupRef.current = true;
    const group = await createTriggerGroup(trimmed);
    if (group) showToast(`Group "${group.name}" created`, 'info');
    setNewGroupName(null);
  }

  if (triggersLoadable.status === 'failed') {
    return (
      <div class="content-view active">
        <div class="list-rows">
          <LoadableError noun="triggers" error={triggersLoadable.error} />
        </div>
      </div>
    );
  }

  return (
    <div class="content-view active">
      <div class="list-rows">
        <LoadingFade showSkeleton={showTriggersLoading} skeleton={<ListSkeleton fill />}>
          {triggersLoadable.status === 'loaded' ? (
            <TriggersLoaded
              triggersData={triggersLoadable.data}
              groupsLoadable={groupsLoadable}
              newGroupName={newGroupName}
              setNewGroupName={setNewGroupName}
              commitNewGroup={commitNewGroup}
              creatingGroupRef={creatingGroupRef}
            />
          ) : null}
        </LoadingFade>
      </div>
    </div>
  );
}

function TriggersLoaded({
  triggersData,
  groupsLoadable,
  newGroupName,
  setNewGroupName,
  commitNewGroup,
  creatingGroupRef,
}: {
  triggersData: TriggerInfo[];
  groupsLoadable: typeof triggerGroups.value;
  newGroupName: string | null;
  setNewGroupName: (v: string | null) => void;
  commitNewGroup: () => void;
  creatingGroupRef: { current: boolean };
}) {
  // Group registry is small; if it failed, fall back to a flat panel under
  // "Ungrouped" so the user still sees their triggers. The failure surfaces
  // separately wherever the registry is needed (e.g. the picker in the trigger
  // form). Same idea as triggers' empty-as-failed guard.
  const groups = loadedOr(groupsLoadable, []);
  const knownGroupIds = new Set(groups.map(g => g.id));

  // Bucket triggers by group id (null → ungrouped). A trigger whose group_id
  // doesn't resolve to a known group (e.g. concurrent delete landed between
  // the trigger's group_id update and the panel refetch) falls back to the
  // Ungrouped section so the row can never go invisible.
  const byGroup = new Map<string, TriggerInfo[]>();
  for (const t of triggersData) {
    const key = t.group_id && knownGroupIds.has(t.group_id) ? t.group_id : UNGROUPED_KEY;
    const bucket = byGroup.get(key);
    if (bucket) bucket.push(t);
    else byGroup.set(key, [t]);
  }
  for (const bucket of byGroup.values()) bucket.sort(sortByCompletion);

  const collapsed = collapsedTriggerGroupIds.value;
  const ungroupedTriggers = byGroup.get(UNGROUPED_KEY) ?? [];

  return (
    <>
      {groups.map(group => {
        const members = byGroup.get(group.id) ?? [];
        const isCollapsed = collapsed.has(group.id);
        return (
          <div class="trigger-group-section" key={group.id}>
            <TriggerGroupHeader group={group} />
            {!isCollapsed && members.map(trigger => (
              <TriggerItem key={trigger.id} trigger={trigger} />
            ))}
          </div>
        );
      })}
      {ungroupedTriggers.length > 0 && (
        <div class="trigger-group-section trigger-group-section-ungrouped">
          {groups.length > 0 && (
            <div class="trigger-group-header trigger-group-header-ungrouped">
              <span class="trigger-group-name">Ungrouped</span>
              <span class="trigger-group-count">({ungroupedTriggers.length})</span>
            </div>
          )}
          {ungroupedTriggers.map(trigger => (
            <TriggerItem key={trigger.id} trigger={trigger} />
          ))}
        </div>
      )}
      {newGroupName !== null && (
        <div class="trigger-group-create-row">
          <input
            class="trigger-group-name-input"
            type="text"
            autoFocus
            value={newGroupName}
            placeholder="Group name"
            onInput={e => setNewGroupName((e.target as HTMLInputElement).value)}
            onBlur={commitNewGroup}
            onKeyDown={e => {
              if (e.key === 'Enter') void commitNewGroup();
              else if (e.key === 'Escape') setNewGroupName(null);
            }}
          />
        </div>
      )}
      <div class="trigger-add-row">
        <div class="list-row-add-card" onClick={openAddTrigger}>
          <div class="list-row-add-icon">+</div>
          <div class="list-row-add-label">Add Trigger</div>
        </div>
        <div class="list-row-add-card" onClick={() => { creatingGroupRef.current = false; setNewGroupName(''); }}>
          <div class="list-row-add-icon">+</div>
          <div class="list-row-add-label">New Group</div>
        </div>
      </div>
    </>
  );
}
