import { useEffect, useState } from 'preact/hooks';
import { showToast } from '../../store/store';
import { getCcAllowedTools, putCcAllowedTools } from '../../api/client';
import { errorDetail } from '../../utils/errorDetail';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { toFailed, type Loadable } from '../../store/types';
import { LoadableError } from '../shared/LoadableError';

/** Direct view/edit of `~/.lucidos/cc-allowed-tools` — the file Lucidos passes
 *  to each coding-agent session (today: Claude Code) as `--allowedTools`. Edits
 *  take effect on the next session spawn (running sessions keep their frozen flag). */
export function CcAllowedToolsSection() {
  const [loadable, setLoadable] = useState<Loadable<string>>({ status: 'not-loaded' });
  const [draft, setDraft] = useState<string>('');
  const [saving, setSaving] = useState(false);
  const showLoading = useDelayedLoading(loadable);

  useEffect(() => {
    setLoadable({ status: 'loading' });
    getCcAllowedTools()
      .then((contents) => {
        setLoadable({ status: 'loaded', data: contents });
        setDraft(contents);
      })
      .catch((e) => {
        setLoadable(toFailed(e));
      });
  }, []);

  async function save() {
    setSaving(true);
    try {
      await putCcAllowedTools(draft);
      setLoadable({ status: 'loaded', data: draft });
      showToast('Saved', 'info');
    } catch (e) {
      showToast(`Save failed: ${errorDetail(e)}`, 'error');
    } finally {
      setSaving(false);
    }
  }

  if (loadable.status === 'failed') {
    return (
      <div class="settings-section">
        <div class="settings-section-title">Tool permissions</div>
        <LoadableError noun="tool permissions" error={loadable.error} />
      </div>
    );
  }
  if (loadable.status !== 'loaded') {
    if (!showLoading) return null;
    return (
      <div class="settings-section">
        <div class="settings-section-title">Tool permissions</div>
        <div class="loading-spinner" />
      </div>
    );
  }

  const dirty = draft !== loadable.data;

  return (
    <div class="settings-section">
      <div class="settings-section-title">Tool permissions</div>
      <p class="settings-section-desc">
        Patterns passed to Claude Code as <code>--allowedTools</code>. One per line; lines starting with <code>#</code> are ignored.
        Use the <strong>Always allow</strong> buttons on permission prompts to add entries quickly. Changes apply to new Claude Code sessions.
      </p>
      <textarea
        class="cc-allowed-tools-editor"
        rows={14}
        spellcheck={false}
        value={draft}
        onInput={(e) => setDraft((e.target as HTMLTextAreaElement).value)}
      />
      <div class="cc-allowed-tools-actions">
        <button
          type="button"
          class="action-btn"
          disabled={!dirty || saving}
          onClick={() => setDraft(loadable.data)}
        >
          Revert
        </button>
        <button
          type="button"
          class="action-btn action-btn-confirm"
          disabled={!dirty || saving}
          onClick={save}
        >
          {saving ? 'Saving...' : 'Save'}
        </button>
      </div>
    </div>
  );
}
