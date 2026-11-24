import { useEffect, useState } from 'preact/hooks';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { useVersionedRefresh } from '../../hooks/useVersionedRefresh';
import { preferences, settingsScrollTarget } from '../../store/store';
import { openSettingsSubview } from '../../store/actions/menu';
import {
  currentMemoryModule,
  setMemoryModule,
  storedBackgroundSelection,
  storedMemoryModule,
  type MemoryModule,
} from '../../store/actions/preferences';
import { loadTreeBackfill, treeBackfillStarted, treeBackfillVersion } from '../../store/actions/treeBackfill';
import { displayModelName } from '../../store/thread-events/exchange';
import { Disclosure } from '../shared/Disclosure';
import { PREF_MEMORY_MODULE } from '@lucidos/preference-catalog';
import { TreeBackfillConfirm } from './TreeBackfillConfirm';
import { TreeBackfillStatus } from './TreeBackfillStatus';
import { useBackgroundModels } from './useBackgroundModels';

const MEMORY_MODULE_LABELS: Record<MemoryModule, string> = {
  classic: 'Classic',
  tree: 'Tree',
};

const MEMORY_MODULE_OPTIONS = PREF_MEMORY_MODULE.values.map((value) => ({
  value,
  label: MEMORY_MODULE_LABELS[value],
}));

const SUMMARY_COMPACTION_ANCHOR = 'models:summary-compaction';

export function MemoryModuleSection() {
  const loaded = preferences.value.status === 'loaded';
  const chosen = loaded ? currentMemoryModule() : null;
  const [confirming, setConfirming] = useState(false);
  const [treeButton, setTreeButton] = useState<HTMLButtonElement | null>(null);
  const background = useBackgroundModels();
  const compactor = storedBackgroundSelection('model_summary_compaction', 'reasoning_summary_compaction').model
    ?? background.row('model_summary_compaction')?.model
    ?? null;
  useEffect(() => { void loadTreeBackfill(); }, []);
  useVersionedRefresh(treeBackfillVersion.value, false, () => void loadTreeBackfill());
  usePanelRefresh('Tree memory', chosen === 'tree' ? loadTreeBackfill : null);

  function openCompactionModel() {
    openSettingsSubview('models');
    settingsScrollTarget.value = SUMMARY_COMPACTION_ANCHOR;
  }

  function choose(module: MemoryModule) {
    if (module === 'tree') {
      if (chosen !== 'tree' && treeBackfillStarted()) {
        void startTree();
        return;
      }
      setConfirming(chosen !== 'tree');
      return;
    }
    setConfirming(false);
    if (storedMemoryModule() !== 'classic') void setMemoryModule('classic');
  }

  async function startTree() {
    await setMemoryModule('tree');
    setConfirming(false);
    await loadTreeBackfill();
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title">Memory module</div>
      <div class="settings-row" data-search-anchor="memory:module">
        <span class="settings-row-label">Module</span>
        <div class="segmented-control" role="group" aria-label="Memory module">
          {MEMORY_MODULE_OPTIONS.map((m) => (
            <button
              key={m.value}
              ref={m.value === 'tree' ? setTreeButton : undefined}
              type="button"
              aria-pressed={chosen === m.value}
              aria-haspopup={m.value === 'tree' && chosen !== 'tree' ? 'dialog' : undefined}
              class={`segmented-btn ${chosen === m.value ? 'active' : ''}`}
              onClick={() => choose(m.value)}
            >
              {m.label}
            </button>
          ))}
        </div>
      </div>
      <div class="settings-row-note">
        Classic keeps a running summary of each thread and recalls saved
        memories. Tree gives every turn a layered summary of the workspace and
        the thread, and the agent can zoom in on any line. Tree takes effect
        after a one-time background summary of the workspace. Until then, turns
        stay on Classic. Switching back to Classic keeps what Tree built.
      </div>
      {confirming && chosen !== 'tree' && (
        <TreeBackfillConfirm
          background={background}
          anchor={treeButton}
          onStart={startTree}
          onCancel={() => setConfirming(false)}
        />
      )}
      <Disclosure open={chosen === 'tree'}>
        {chosen === 'tree' && (
          <TreeBackfillStatus onPickModel={openCompactionModel} onSwitchToClassic={() => void setMemoryModule('classic')} />
        )}
        <div class="settings-row-note" data-role="memory-compaction-link">
          {compactor ? `${displayModelName(compactor)} writes` : 'A background model writes'}{' '}
          Tree's summaries. Change it under{' '}
          <button type="button" class="accent-link" onClick={openCompactionModel}>
            Models → Background tasks
          </button>
          .
        </div>
      </Disclosure>
    </div>
  );
}
