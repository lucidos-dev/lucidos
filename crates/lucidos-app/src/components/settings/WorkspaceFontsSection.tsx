import { useEffect, useState } from 'preact/hooks';
import { WORKSPACE_FONT_LIMITS, type FontGroup, type WorkspaceFont } from '@lucidos/appearance';
import { registerWorkspaceFont } from '@lucidos/font-faces';
import { dataMountUrl, type InvalidWorkspaceFont } from '../../api/client';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { showConfirm, showToast } from '../../store/store';
import { setFontFamily } from '../../store/actions/preferences';
import {
  guessFace,
  installWorkspaceFont,
  loadWorkspaceFonts,
  removeWorkspaceFont,
  workspaceFontList,
  type FontFileChoice,
} from '../../store/actions/workspaceFonts';
import { errorDetail } from '../../utils/errorDetail';
import { Disclosure } from '../shared/Disclosure';
import { Dropdown } from '../shared/Dropdown';
import { Explainer } from '../shared/Explainer';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, SkBlock, SkText } from '../shared/Skeleton';
import { FONT_GROUP_LABELS } from './fontOptions';

const FONT_FILE_TYPES = '.woff2,.woff,.ttf,.otf';

const GROUP_OPTIONS = (Object.keys(FONT_GROUP_LABELS) as FontGroup[]).map(group => ({
  value: group,
  label: FONT_GROUP_LABELS[group],
}));

const STYLE_OPTIONS = [
  { value: 'normal', label: 'Normal' },
  { value: 'italic', label: 'Italic' },
];

/** What a font can be, read from its group: a mono font also sets code. */
function fitsLabel(group: FontGroup): string {
  return group === 'mono' ? 'UI and code' : 'UI';
}

/** Settings → Appearance → Workspace fonts: install a font file into the
 *  workspace, then pick it like any other font (ADR 0308). */
export function WorkspaceFontsSection() {
  const list = workspaceFontList.value;
  const showLoading = useDelayedLoading(list);
  const [installing, setInstalling] = useState(false);

  useEffect(() => {
    if (workspaceFontList.value.status === 'not-loaded') void loadWorkspaceFonts();
  }, []);
  usePanelRefresh('workspace fonts', loadWorkspaceFonts);

  return (
    <>
      <div class="settings-row" data-search-anchor="appearance:workspace-fonts">
        <span class="settings-row-label">
          Workspace fonts
          <Explainer title="Workspace fonts">
            <p>
              Install your own font files (woff2, woff, TrueType or OpenType) and
              pick them under Font. A mono font can also be a theme's code font.
            </p>
            <p>
              The files live in your workspace under data/fonts and load from it,
              never from the internet. You can also ask the agent to install one.
            </p>
          </Explainer>
        </span>
        <button class="settings-option" onClick={() => setInstalling(open => !open)}>
          {installing ? 'Cancel' : 'Install font'}
        </button>
      </div>
      <Disclosure open={installing}>
        <InstallFontForm onInstalled={() => setInstalling(false)} />
      </Disclosure>
      {list.status === 'failed' ? (
        <LoadableError noun="workspace fonts" error={list.error} onRetry={() => void loadWorkspaceFonts()} />
      ) : (
        <LoadingFade
          showSkeleton={showLoading}
          skeleton={<ListSkeletonOf count={2} containerClass="workspace-font-rows" row={() => <WorkspaceFontRow />} />}
        >
          {list.status === 'loaded' && (list.data.fonts.length > 0 || list.data.invalid.length > 0) && (
            <div class="workspace-font-rows">
              {list.data.fonts.map(font => <WorkspaceFontRow key={font.id} font={font} />)}
              {list.data.invalid.map(broken => <BrokenFontRow key={broken.id} broken={broken} />)}
            </div>
          )}
        </LoadingFade>
      )}
    </>
  );
}

async function confirmAndRemove(id: string, name: string): Promise<void> {
  const ok = await showConfirm(`Remove the font "${name}" from this workspace?`, 'Remove', { variant: 'danger' });
  if (!ok) return;
  try {
    await removeWorkspaceFont(id);
  } catch (e) {
    showToast(`Could not remove the font "${name}": ${errorDetail(e)}`, 'error');
  }
}

/** One installed font, its name set in the font itself. Self-skeletonizing. */
function WorkspaceFontRow({ font }: { font?: WorkspaceFont }) {
  useEffect(() => {
    if (font) registerWorkspaceFont(font, dataMountUrl);
  }, [font]);
  return (
    <div class="workspace-font-row">
      <div class="workspace-font-info">
        <SkText class="workspace-font-name" w="8rem">
          <span style={font ? { fontFamily: font.stack } : undefined}>{font?.label}</span>
        </SkText>
        <SkText class="workspace-font-meta" w="5rem">
          {font && `${FONT_GROUP_LABELS[font.group]} · ${fitsLabel(font.group)}`}
        </SkText>
      </div>
      <SkBlock w="6.5rem" h="1.5rem" round>
        <div class="settings-row-options">
          <button class="settings-option" onClick={() => font && void setFontFamily(font.id)}>Use</button>
          <button class="settings-option" onClick={() => font && void confirmAndRemove(font.id, font.label)}>
            Remove
          </button>
        </div>
      </SkBlock>
    </div>
  );
}

/** A font directory the engine refused, with its reason, so it can be fixed or
 *  removed rather than silently missing from the picker. */
function BrokenFontRow({ broken }: { broken: InvalidWorkspaceFont }) {
  return (
    <div class="workspace-font-row">
      <div class="workspace-font-info">
        <span class="workspace-font-name">{broken.id}</span>
        <span class="settings-field-error">{broken.reason}</span>
      </div>
      <button class="settings-option" onClick={() => void confirmAndRemove(broken.id, broken.id)}>
        Remove
      </button>
    </div>
  );
}

/** A label from the first file's name: `BrandSans-Bold.woff2` is `BrandSans`. */
function labelFromFileName(name: string): string {
  return name.replace(/\.[^.]+$/, '').split(/[-_]/)[0] ?? '';
}

function InstallFontForm({ onInstalled }: { onInstalled: () => void }) {
  const [label, setLabel] = useState('');
  const [group, setGroup] = useState<FontGroup>('sans');
  const [ligatures, setLigatures] = useState(false);
  const [files, setFiles] = useState<FontFileChoice[]>([]);
  const [busy, setBusy] = useState(false);

  const chooseFiles = (list: FileList | null) => {
    const chosen = Array.from(list ?? []).map(file => ({ file, ...guessFace(file.name) }));
    setFiles(chosen);
    if (!label.trim() && chosen[0]) setLabel(labelFromFileName(chosen[0].file.name));
  };

  const updateFile = (index: number, patch: Partial<FontFileChoice>) => {
    setFiles(current => current.map((choice, i) => (i === index ? { ...choice, ...patch } : choice)));
  };

  const install = async () => {
    setBusy(true);
    try {
      const id = await installWorkspaceFont({ label, group, ligatures, files });
      const name = label.trim();
      showToast(`Installed the font "${name}".`, 'success', {
        action: { label: 'Use it', onClick: () => void setFontFamily(id) },
      });
      setLabel('');
      setFiles([]);
      onInstalled();
    } catch (e) {
      showToast(`Could not install the font "${label.trim()}": ${errorDetail(e)}`, 'error');
    } finally {
      setBusy(false);
    }
  };

  return (
    <div class="workspace-font-form">
      <div class="settings-row settings-row-child">
        <span class="settings-row-label">Files</span>
        <input
          type="file"
          multiple
          accept={FONT_FILE_TYPES}
          onChange={e => chooseFiles((e.currentTarget as HTMLInputElement).files)}
        />
      </div>
      {files.map((choice, i) => (
        <div class="settings-row settings-row-child workspace-font-file" key={`${choice.file.name}-${i}`}>
          <span class="settings-row-label">{choice.file.name}</span>
          <div class="settings-row-options">
            <input
              class="settings-text-input workspace-font-weight"
              aria-label={`Weight of ${choice.file.name}`}
              value={choice.weight}
              onInput={e => updateFile(i, { weight: (e.currentTarget as HTMLInputElement).value })}
            />
            <Dropdown
              options={STYLE_OPTIONS}
              value={choice.style}
              onChange={v => updateFile(i, { style: v as FontFileChoice['style'] })}
            />
          </div>
        </div>
      ))}
      <div class="settings-row settings-row-child">
        <span class="settings-row-label">Name</span>
        <input
          class="settings-text-input"
          value={label}
          maxLength={WORKSPACE_FONT_LIMITS.label}
          placeholder="Brand Sans"
          onInput={e => setLabel((e.currentTarget as HTMLInputElement).value)}
        />
      </div>
      <div class="settings-row settings-row-child">
        <span class="settings-row-label">Group</span>
        <Dropdown options={GROUP_OPTIONS} value={group} onChange={v => setGroup(v as FontGroup)} />
      </div>
      {group === 'mono' && (
        <div class="settings-row settings-row-child">
          <span class="settings-row-label">Programming ligatures</span>
          <label class="toggle-switch">
            <input
              type="checkbox"
              checked={ligatures}
              onChange={e => setLigatures((e.currentTarget as HTMLInputElement).checked)}
            />
            <span class="toggle-slider" />
          </label>
        </div>
      )}
      <div class="settings-row settings-row-child">
        <span class="settings-row-note">
          Weight is one number, or a range such as 100 900 for a variable font.
        </span>
        <button class="settings-option" disabled={busy || files.length === 0} onClick={() => void install()}>
          {busy ? 'Installing…' : 'Install'}
        </button>
      </div>
    </div>
  );
}
