import { useEffect, useState } from 'preact/hooks';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { preferences, settingsScrollTarget } from '../../store/store';
import { openSettingsSubview } from '../../store/actions/menu';
import {
  currentMemoryModule,
  setMemoryModule,
  storedBackgroundSelection,
  type MemoryModule,
} from '../../store/actions/preferences';
import { loadTreeBackfill, treeBackfillStarted } from '../../store/actions/treeBackfill';
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

/** The anchor of the compactor's model row on Settings → Models. */
const SUMMARY_COMPACTION_ANCHOR = 'models:summary-compaction';

/** Heads Settings → System → Memory: how a turn gets its past (ADR 0362).
 *
 *  Classic is written at once. Tree costs money, so the first time its button
 *  only opens the confirm modal: the estimate and the compactor model, and the
 *  Start button that writes `memory_module`. Once a backfill has started,
 *  Tree is written at once too, and the compactor resumes it. */
export function MemoryModuleSection() {
  // Nothing reads as pressed until preferences load, so a Tree workspace never
  // shows Classic first.
  const loaded = preferences.value.status === 'loaded';
  const chosen = loaded ? currentMemoryModule() : null;
  const [confirming, setConfirming] = useState(false);
  const [treeButton, setTreeButton] = useState<HTMLButtonElement | null>(null);
  const background = useBackgroundModels();
  const compactor = storedBackgroundSelection('model_summary_compaction', 'reasoning_summary_compaction').model
    ?? background.row('model_summary_compaction')?.model
    ?? null;
  // Read on Classic too: it says whether choosing Tree resumes a backfill.
  useEffect(() => { void loadTreeBackfill(); }, []);
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
    if (chosen !== 'classic') void setMemoryModule('classic');
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
        {chosen === 'tree' && <TreeBackfillStatus onPickModel={openCompactionModel} />}
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
