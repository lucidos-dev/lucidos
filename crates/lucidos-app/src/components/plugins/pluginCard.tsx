import type { MarketplacePlugin } from '../../store/types';
import { useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { GlyphBadge } from '../shared/GlyphBadge';
import { AppIcon } from '../shared/AppIcon';
import { openAppById } from '../../store/actions/apps';
import { pluginMediaUrl } from '../../api/client';
import { focusSpawnedThread } from '../../store/actions/threads';
import { uninstallMarketplacePlugin } from '../../store/actions/plugin-uninstall';
import { ProposeUpstreamButton } from './ProposeUpstreamButton';
import { contentLabel } from './pluginContent';
import { NO_ENGINE_REQUIREMENT_CHIP, NO_ENGINE_REQUIREMENT_SENTENCE } from './engineRequirement';

/** The parts a Plugins panel row and the plugin detail page share, so the two
 *  cannot disagree about a plugin's status, chips or buttons. The row shows
 *  a subset; the detail page shows all of it. */

export function statusLabel(plugin: MarketplacePlugin): string {
  switch (plugin.status) {
    case 'installed': return `Installed v${plugin.installed_version ?? plugin.version}`;
    case 'update_available': return `Update from v${plugin.installed_version} to v${plugin.version}`;
    case 'available': return `v${plugin.version}`;
  }
}

function actionLabel(plugin: MarketplacePlugin): string {
  return plugin.status === 'update_available' ? 'Update' : 'Install';
}

/** The card's primary button. Progresses Install/Update → Setup → Open:
 *  - not installed → Install (or Update for an out-of-date install), disabled
 *    with the engine's reason when this Lucidos cannot install that version
 *  - installed with an unfinished setup thread → Setup (opens that thread)
 *  - installed and setup done (or none) with an app → Open (launches it)
 *  - installed with nothing to open → no button; the row says Installed
 *  An out-of-date install always shows Update first, before Setup/Open. */
type CardAction =
  | { kind: 'install'; label: string; blockedReason?: string }
  | { kind: 'setup'; threadId: string }
  | { kind: 'open'; appId: string }
  | { kind: 'none' };

export function cardPrimaryAction(plugin: MarketplacePlugin): CardAction {
  if (plugin.status !== 'installed') {
    return {
      kind: 'install',
      label: actionLabel(plugin),
      blockedReason: plugin.engine_compatible ? undefined : plugin.engine_incompatible_reason,
    };
  }
  if (plugin.setup_thread_id && !plugin.setup_complete) {
    return { kind: 'setup', threadId: plugin.setup_thread_id };
  }
  if (plugin.app_id) return { kind: 'open', appId: plugin.app_id };
  return { kind: 'none' };
}

export function fileCountLabel(count: number): string {
  return `${count} ${count === 1 ? 'file' : 'files'}`;
}

/** Tooltip for the "Modified" badge, listing the changed paths (capped so the
 *  tooltip stays readable). An update merges these changes, so the tooltip
 *  must never warn that it overwrites them. */
function modifiedTooltip(paths?: string[]): string {
  const base = 'You have locally changed this plugin. An update merges your changes into the new version where it can.';
  if (!paths || paths.length === 0) return base;
  const shown = paths.slice(0, 6).join(', ');
  const more = paths.length > 6 ? `, +${paths.length - 6} more` : '';
  return `${base} Changed: ${shown}${more}`;
}

/** Title-case a kebab-case category id for display: `developer-tools` →
 *  `Developer tools`. */
export function categoryLabel(category: string): string {
  const spaced = category.replace(/-/g, ' ');
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** The plugin icon: its own, its single app's, or the monogram tile. */
export function PluginIcon({ plugin }: { plugin?: MarketplacePlugin }) {
  return (
    <SkBlock w="var(--app-icon-size)" h="var(--app-icon-size)" round>
      {plugin && <AppIcon appId={plugin.id} name={plugin.name} src={pluginIconSrc(plugin)} />}
    </SkBlock>
  );
}

function pluginIconSrc(plugin: MarketplacePlugin): string | undefined {
  const url = plugin.media.icon_url;
  return url ? pluginMediaUrl(url) : undefined;
}

/** The detail page's name, with the version and Modified badges beside it.
 *  A Store row shows the name alone. */
export function PluginTitle({ plugin, nameClass }: { plugin?: MarketplacePlugin; nameClass: string }) {
  const sk = useSkeleton();
  return (
    <div class="app-store-plugin-title-row">
      <SkText class={nameClass} w="9rem">{plugin?.name}</SkText>
      {(sk || plugin) && (
        <SkBlock w="5rem" h="1rem" round>
          <GlyphBadge class={`app-store-status app-store-status-${plugin?.status}`}>
            {plugin && statusLabel(plugin)}
          </GlyphBadge>
        </SkBlock>
      )}
      {plugin?.modified && (
        <GlyphBadge class="app-store-modified-chip" data-tooltip={modifiedTooltip(plugin.modified_paths)}>
          Modified
        </GlyphBadge>
      )}
    </div>
  );
}

/** Why this Lucidos cannot install the plugin, beside the disabled Install. */
function engineBlockedReason(plugin?: MarketplacePlugin): string | undefined {
  return plugin && !plugin.engine_compatible && plugin.status !== 'installed'
    ? plugin.engine_incompatible_reason
    : undefined;
}

function EngineRequirementChip({ reason }: { reason: string }) {
  return (
    <span class="label label-warning" data-role="engine-requirement">
      {reason}
    </span>
  );
}

/** The detail page's line under the name: where the plugin comes from and how
 *  many files it has. */
export function PluginSource({ plugin }: { plugin?: MarketplacePlugin }) {
  const source = plugin
    ? [plugin.marketplace_name, fileCountLabel(plugin.files_count)].filter(Boolean).join(' · ')
    : undefined;
  return <SkText class="plugin-detail-source" as="div" w="10rem">{source}</SkText>;
}

/** The detail page's content and category chips, and the engine notes. The
 *  no-requirement note informs an install, so an installed plugin drops it. */
export function PluginMeta({ plugin }: { plugin?: MarketplacePlugin }) {
  const sk = useSkeleton();
  const blockedReason = engineBlockedReason(plugin);
  return (
    <div class="app-store-plugin-meta">
      {sk && <SkBlock w="4rem" h="1rem" round />}
      {plugin?.content.map((kind) => (
        <span class="label label-neutral" key={kind}>
          {contentLabel(kind)}
        </span>
      ))}
      {plugin?.categories.map((c) => (
        <span class="label label-neutral" key={`cat-${c}`}>
          {categoryLabel(c)}
        </span>
      ))}
      {blockedReason && <EngineRequirementChip reason={blockedReason} />}
      {plugin && plugin.status !== 'installed' && plugin.engine_requirement == null && (
        <span
          class="label label-neutral"
          data-role="engine-undeclared"
          data-tooltip={NO_ENGINE_REQUIREMENT_SENTENCE}
        >
          {NO_ENGINE_REQUIREMENT_CHIP}
        </span>
      )}
    </div>
  );
}

/** The row keeps one note: why its Install is disabled. Every other chip waits
 *  on the detail page, so the list stays calm. */
export function PluginRowNote({ plugin }: { plugin?: MarketplacePlugin }) {
  const blockedReason = engineBlockedReason(plugin);
  if (!blockedReason) return null;
  return (
    <div class="app-store-plugin-meta">
      <EngineRequirementChip reason={blockedReason} />
    </div>
  );
}

interface PluginActionsProps {
  plugin?: MarketplacePlugin;
  /** True while this plugin's install is being staged. */
  busy?: boolean;
  onInstall?: (plugin: MarketplacePlugin) => void;
}

interface PrimaryButton {
  label: string;
  onClick: () => void;
  blockedReason?: string;
  /** Green, so an outdated plugin stands out from Install and Open. */
  confirm?: boolean;
}

function primaryButton(
  plugin: MarketplacePlugin | undefined,
  onInstall: PluginActionsProps['onInstall'],
): PrimaryButton | null {
  const action = plugin ? cardPrimaryAction(plugin) : { kind: 'none' as const };
  switch (action.kind) {
    case 'install':
      return {
        label: action.label,
        onClick: () => plugin && onInstall?.(plugin),
        blockedReason: action.blockedReason,
        confirm: plugin?.status === 'update_available',
      };
    case 'setup':
      // The catalog surfaces this button for a present-or-queued setup thread;
      // a gone one resolves to Open. A QUEUED one has no thread_summaries row
      // at all. So a bootstrap fetch would 404, and a plain focusThread would
      // be undone by ThreadView's stale-pointer cleanup and land on the compose
      // view. `focusSpawnedThread` holds the focus until the row arrives,
      // exactly as the confirm path in plugin-install.ts does.
      return { label: 'Setup', onClick: () => focusSpawnedThread(action.threadId) };
    case 'open':
      return { label: 'Open', onClick: () => void openAppById(action.appId) };
    case 'none':
      return null;
  }
}

function PrimaryActionButton({ plugin, busy = false, onInstall }: PluginActionsProps) {
  const sk = useSkeleton();
  const primary = primaryButton(plugin, onInstall);
  if (!sk && !primary) return null;
  return (
    <SkBlock w="4.5rem" h="2rem" round>
      <button
        class={`action-btn${primary?.confirm ? ' action-btn-confirm' : ''}`}
        type="button"
        disabled={busy || !!primary?.blockedReason}
        onClick={primary?.onClick}
      >
        {busy ? 'Staging' : primary?.label}
      </button>
    </SkBlock>
  );
}

/** A Plugins panel row's one button: Install, Update, Setup or Open. With
 *  nothing to press, the row says Installed, since it shows no version badge.
 *  A tap never reaches the row behind it, which opens the plugin detail page. */
export function PluginRowAction(props: PluginActionsProps) {
  const { plugin } = props;
  return (
    <div class="list-row-actions" onClick={(e) => e.stopPropagation()}>
      {plugin && cardPrimaryAction(plugin).kind === 'none'
        ? <span class="app-store-plugin-installed">Installed</span>
        : <PrimaryActionButton {...props} />}
    </div>
  );
}

/** The detail page's buttons: Uninstall and the primary. */
export function PluginActions(props: PluginActionsProps) {
  const { plugin } = props;
  const isInstalled = !!plugin && plugin.status !== 'available';
  return (
    <div class="list-row-actions">
      {isInstalled && (
        <button
          class="action-btn action-btn-secondary"
          type="button"
          onClick={() => plugin && void uninstallMarketplacePlugin(plugin)}
        >
          Uninstall
        </button>
      )}
      <PrimaryActionButton {...props} />
    </div>
  );
}

/** The detail page's note on a locally changed plugin, with the offer to send
 *  the changes to its author. */
export function PluginModifiedNotice({ plugin }: { plugin: MarketplacePlugin }) {
  if (!plugin.modified) return null;
  const count = plugin.modified_paths?.length ?? 0;
  const what = count > 0 ? `${fileCountLabel(count)} in this plugin` : 'this plugin';
  return (
    <div class="plugin-detail-modified">
      <span>You changed {what}. An update keeps your changes where it can.</span>
      <ProposeUpstreamButton pluginId={plugin.id} pluginName={plugin.name} />
    </div>
  );
}
