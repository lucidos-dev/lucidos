import { useEffect, useMemo, useRef, useState } from 'preact/hooks';
import { installedPlugins, marketplaceCatalog } from '../../store/store';
import type { Loadable, MarketplacePlugin, MediaProblem } from '../../store/types';
import { fetchPluginReadme, pluginMediaUrl } from '../../api/client';
import { useDelayedFlag } from '../../hooks/useDelayedLoading';
import { useLoadableFetch } from '../../hooks/useLoadableFetch';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { refreshPluginCatalog } from '../../store/actions/plugin-marketplaces';
import { loadInstalledPlugins } from '../../store/actions/plugins';
import { installMarketplacePlugin } from '../../store/actions/plugin-install';
import { openImageGallery } from '../../store/imagePopup';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { SkeletonProvider, useSkeleton } from '../shared/Skeleton';
import { ProseSkeleton } from '../files/previewSkeletons';
import { pluginRowsInScope, pluginRowsSettled, refreshPluginsPanel } from './StoreTab';
import {
  PluginActions, PluginIcon, PluginMeta, PluginModifiedNotice, PluginSource, PluginTitle,
} from './pluginCard';
import { renderPluginReadme } from './pluginReadme';
import { markdownEngine } from '../../utils/renderMarkdown';
import { PluginDescription } from './PluginDescription';

interface PluginDetailPageProps {
  marketplaceId: string;
  pluginId: string;
  name: string;
}

export function PluginDetailPage({ marketplaceId, pluginId, name }: PluginDetailPageProps) {
  const catalog = marketplaceCatalog.value;
  const installed = installedPlugins.value;
  const settled = pluginRowsSettled(catalog, installed);
  const showLoading = useDelayedFlag(!settled);

  useEffect(() => {
    void refreshPluginCatalog();
    void loadInstalledPlugins();
  }, []);

  const plugin = pluginRowsInScope(catalog, installed, false)
    .find((p) => p.marketplace_id === marketplaceId && p.id === pluginId);

  return (
    <div class="plugin-detail">
      <LoadingFade
        showSkeleton={showLoading}
        skeleton={<SkeletonProvider><div class="plugin-detail-column"><PluginDetailHead /></div></SkeletonProvider>}
      >
        {settled ? <PluginDetailBody plugin={plugin} name={name} /> : null}
      </LoadingFade>
    </div>
  );
}

function PluginDetailBody({ plugin, name }: { plugin?: MarketplacePlugin; name: string }) {
  const catalog = marketplaceCatalog.value;
  const installed = installedPlugins.value;
  const readme = useReadme(plugin?.media.readme_url);
  usePanelRefresh('plugin detail', () => Promise.all([refreshPluginsPanel(), readme.reload()]));

  if (!plugin) {
    const failed = catalog.status === 'failed' ? catalog : installed.status === 'failed' ? installed : null;
    if (failed) return <LoadableError noun="plugin catalog" error={failed.error} />;
    return (
      <div class="empty-state">
        <p>{name} is no longer in a marketplace, and it is not installed.</p>
      </div>
    );
  }
  return (
    <div class="plugin-detail-column">
      <PluginDetailHead plugin={plugin} />
      <PluginModifiedNotice plugin={plugin} />
      <MediaStrip plugin={plugin} />
      {plugin.media.readme_url && <Readme readme={readme.loadable} showLoading={readme.showLoading} />}
      <MediaProblems problems={plugin.media.problems} />
    </div>
  );
}

function PluginDetailHead({ plugin }: { plugin?: MarketplacePlugin }) {
  const sk = useSkeleton();
  const [busy, setBusy] = useState(false);
  async function stageInstall(p: MarketplacePlugin) {
    setBusy(true);
    try {
      await installMarketplacePlugin(p);
    } finally {
      setBusy(false);
    }
  }
  return (
    <div class="plugin-detail-intro">
      <div class="plugin-detail-head">
        <div class="plugin-detail-icon">
          <PluginIcon plugin={plugin} />
        </div>
        <div class="plugin-detail-heading">
          <PluginTitle plugin={plugin} nameClass="title plugin-detail-name" />
          <PluginSource plugin={plugin} />
        </div>
        <PluginActions plugin={plugin} busy={busy} onInstall={(p) => void stageInstall(p)} />
      </div>
      {(sk || plugin?.description) && (
        <PluginDescription class="plugin-detail-description" w="20rem" description={plugin?.description} />
      )}
      <PluginMeta plugin={plugin} />
    </div>
  );
}

function MediaStrip({ plugin }: { plugin: MarketplacePlugin }) {
  const shots = plugin.media.screenshots.map(pluginMediaUrl);
  const videos = plugin.media.videos.map(pluginMediaUrl);
  if (shots.length === 0 && videos.length === 0) return null;
  return (
    <ul class="plugin-detail-strip" aria-label={`${plugin.name} screenshots and videos`}>
      {shots.map((src, i) => (
        <li key={src}>
          <button
            type="button"
            class="plugin-detail-shot"
            aria-label={`Screenshot ${i + 1} of ${shots.length}, full size`}
            onClick={() => openImageGallery(shots, i)}
          >
            <img src={src} alt="" loading="lazy" draggable={false} />
          </button>
        </li>
      ))}
      {videos.map((src) => (
        <li key={src}>
          <video class="plugin-detail-video" src={src} controls preload="metadata" />
        </li>
      ))}
    </ul>
  );
}

function Readme({ readme, showLoading }: { readme: Loadable<string>; showLoading: boolean }) {
  const markdown = markdownEngine.value;
  const html = useMemo(
    () => (readme.status === 'loaded' ? renderPluginReadme(readme.data) : ''),
    [readme, markdown],
  );
  if (readme.status === 'failed') {
    return <LoadableError noun="plugin description" error={readme.error} />;
  }
  return (
    <LoadingFade showSkeleton={showLoading} skeleton={<ProseSkeleton class="markdown-content plugin-detail-readme" />}>
      {readme.status === 'loaded' ? (
        <div class="markdown-content plugin-detail-readme authored-html" dangerouslySetInnerHTML={{ __html: html }} />
      ) : null}
    </LoadingFade>
  );
}

function MediaProblems({ problems }: { problems: MediaProblem[] }) {
  if (problems.length === 0) return null;
  const count = problems.length === 1 ? '1 media file was' : `${problems.length} media files were`;
  return (
    <div class="plugin-detail-problems">
      <p>{count} left out:</p>
      <ul>
        {problems.map((p) => (
          <li key={`${p.path}:${p.reason}`}><code>{p.path}</code> {p.reason}.</li>
        ))}
      </ul>
    </div>
  );
}

function useReadme(url: string | undefined) {
  const [epoch, setEpoch] = useState(0);
  const settles = useRef<Array<() => void>>([]);
  const { loadable, showLoading } = useLoadableFetch(
    () => (url ? fetchPluginReadme(url) : Promise.resolve('')),
    [url, epoch],
    {
      keepLoadedWhileRefetching: true,
      onSettled: () => {
        const waiting = settles.current;
        settles.current = [];
        for (const settle of waiting) settle();
      },
    },
  );
  const reload = () => new Promise<void>((resolve) => {
    if (!url) return resolve();
    settles.current.push(resolve);
    setEpoch((n) => n + 1);
  });
  return { loadable, showLoading, reload };
}
