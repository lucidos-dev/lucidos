import { useEffect, useState } from 'preact/hooks';
import {
  appUpdateCheckInFlight,
  appUpdateProgress,
  changelogReleases,
  latestTauriAppNotes,
  lucidosRelease,
  lucidosReleaseDirty,
  whatsNewTargetRelease,
} from '../../store/store';
import { loadChangelog, markWhatsNewSeen } from '../../store/actions/whatsNew';
import {
  followUpdateRoute,
  updateControlLabel,
  updateRoute,
  type UpdateRoute,
} from '../../store/actions/app-update';
import { packagedUpdateVersion } from '../../store/packagedUpdate';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { usePanelRefresh } from '../../hooks/usePanelRefresh';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { isNewerVersion } from '../../utils/version';
import { lucidosVersionTooltip } from '../../utils/lucidosVersion';
import { Disclosure } from '../shared/Disclosure';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { ChevronDownIcon, ChevronRightIcon } from '../shared/icons';
import type { VNode } from 'preact';
import type { ChangelogRelease } from '../../api/client';

export function releaseRowIsOpen(
  version: string,
  openByDefault: boolean,
  toggled: Record<string, boolean>,
): boolean {
  return toggled[version] ?? openByDefault;
}

export function defaultOpenRelease(
  target: string | null,
  running: string | null,
  known: readonly ChangelogRelease[],
): string | null {
  if (target && known.some((r) => r.version === target)) return target;
  return running;
}

interface ReleaseMark {
  label: string;
  tooltip?: string;
  kind: ReleaseRowMarkKind;
}

export type ReleaseRowMarkKind = Exclude<ReleaseRowStatus, 'none'>;

export type ReleaseRowStatus = 'available' | 'newer' | 'running' | 'none';

export function releaseRowStatus(
  version: string,
  running: string | null,
  offered: string | null,
): ReleaseRowStatus {
  if (offered && version === offered) return 'available';
  if (!running) return 'none';
  if (version === running) return 'running';
  return isNewerVersion(version, running) ? 'newer' : 'none';
}

export function leadAheadVersion(
  releases: readonly ChangelogRelease[],
  running: string | null,
  offered: string | null,
): string | null {
  if (offered && releases.some((r) => r.version === offered)) return offered;
  const lead = releases.find((r) => {
    const status = releaseRowStatus(r.version, running, offered);
    return status === 'available' || status === 'newer';
  });
  return lead?.version ?? null;
}

export function releaseRowRoute(
  status: ReleaseRowStatus,
  isLead: boolean,
  route: UpdateRoute,
): UpdateRoute | null {
  if (!isLead) return null;
  return status === 'available' || status === 'newer' ? route : null;
}

export function releaseRowMark(
  status: ReleaseRowStatus,
  route: UpdateRoute | null,
): ReleaseRowMarkKind | null {
  if (status === 'none' || route === 'install') return null;
  return status;
}

function ReleaseRow({
  release,
  mark,
  action,
  open = false,
  onToggle,
}: {
  release?: ChangelogRelease;
  mark?: ReleaseMark;
  action?: VNode | null;
  open?: boolean;
  onToggle?: () => void;
}) {
  const sk = useSkeleton();

  if (sk || !release) {
    return (
      <div class="whats-new-release">
        <div class="whats-new-release-header">
          <SkBlock w="1rem" h="1rem" />
          <SkText w="3.5rem" />
          <SkText w="5rem" />
        </div>
      </div>
    );
  }

  return (
    <div class={`whats-new-release${mark ? ` is-${mark.kind}` : ''}`} data-release={release.version}>
      <div class="whats-new-release-row">
        <button
          type="button"
          class="whats-new-release-header"
          aria-expanded={open}
          onClick={onToggle}
        >
          <span class="whats-new-chevron" aria-hidden="true">
            {open ? <ChevronDownIcon size="1rem" /> : <ChevronRightIcon size="1rem" />}
          </span>
          <span class="whats-new-version">{release.version}</span>
          {release.date && <span class="whats-new-date">{release.date}</span>}
          {mark && (
            <span class={`whats-new-mark is-${mark.kind}`} data-tooltip={mark.tooltip}>
              {mark.label}
            </span>
          )}
        </button>
        {action}
      </div>
      {releaseNotesBody(release, open)}
    </div>
  );
}

export function releaseNotesBody(release: ChangelogRelease, open: boolean): VNode {
  return (
    <Disclosure open={open}>
      <ReleaseNotes release={release} />
    </Disclosure>
  );
}

export function ReleaseNotes({ release }: { release: ChangelogRelease }) {
  return (
    <div
      class="markdown-content whats-new-notes authored-html"
      dangerouslySetInnerHTML={{ __html: renderMarkdown(release.notes) }}
    />
  );
}

export function stripReleaseHeading(
  notes: string,
): { version: string | null; date: string | null; body: string } {
  const lines = notes.replace(/^\s*\n/, '').split('\n');
  const head = lines[0] ?? '';
  const rest = head.startsWith('## v') && /^\d/.test(head.slice(4)) ? head.slice(4) : null;
  if (rest === null) return { version: null, date: null, body: notes.trim() };
  const date = rest.replace(/^\S+/, '').replace(/^[^\p{L}\p{N}]+/u, '').trimEnd();
  return {
    version: /^\S+/.exec(rest)?.[0] ?? null,
    date: date || null,
    body: lines.slice(1).join('\n').trim(),
  };
}

export function offeredRelease(
  version: string | null,
  notes: string | null,
  known: readonly ChangelogRelease[] = [],
): ChangelogRelease | null {
  if (!notes) return null;
  const { version: named, date, body } = stripReleaseHeading(notes);
  const offered = named ?? version;
  if (!offered || !body) return null;
  if (known.some((r) => r.version === offered)) return null;
  return { version: offered, date, notes: body };
}

export function WhatsNewPage() {
  usePanelRefresh("what's new", loadChangelog);
  const loadable = changelogReleases.value;
  const release = lucidosRelease.value;
  const dirty = lucidosReleaseDirty.value;
  const showSkeleton = useDelayedLoading(loadable);
  const [toggled, setToggled] = useState<Record<string, boolean>>({});
  const [target, setTarget] = useState<string | null>(null);

  useEffect(() => {
    void loadChangelog();
  }, []);

  useEffect(() => {
    const requested = whatsNewTargetRelease.value;
    if (!requested) return;
    whatsNewTargetRelease.value = null;
    setTarget(requested);
  }, [whatsNewTargetRelease.value]);

  useEffect(() => {
    if (!target || loadable.status !== 'loaded') return;
    const row = document.querySelector(`[data-release="${CSS.escape(target)}"]`);
    row?.scrollIntoView({ block: 'nearest' });
  }, [target, loadable.status]);

  useEffect(() => {
    if (loadable.status === 'loaded') markWhatsNewSeen(release);
  }, [release, loadable.status]);

  if (loadable.status === 'failed') {
    return <LoadableError error={loadable.error} noun="the changelog" onRetry={() => { void loadChangelog(); }} />;
  }

  const releases = loadable.status === 'loaded' ? loadable.data : [];
  const offeredVersion = packagedUpdateVersion();
  const offered = offeredRelease(offeredVersion, latestTauriAppNotes.value, releases);
  const route = updateRoute(true);
  const lead = offered?.version ?? leadAheadVersion(releases, release, offeredVersion);
  const installing = appUpdateProgress.value !== null;
  const checking = appUpdateCheckInFlight.value;
  const hasCurrent = releases.some((r) => r.version === release);
  const openRelease = defaultOpenRelease(target, release, releases);
  const runningMark: ReleaseMark = {
    kind: 'running',
    label: dirty ? 'Running *' : 'Running',
    tooltip: lucidosVersionTooltip(release, dirty),
  };

  function markFor(kind: ReleaseRowMarkKind, version: string): ReleaseMark {
    if (kind === 'running') return runningMark;
    if (kind === 'available') {
      return {
        kind,
        label: 'Available',
        tooltip: `Lucidos ${version} is available to install`,
      };
    }
    return {
      kind,
      label: 'Newer',
      tooltip: `Lucidos ${version} is published, and you are running ${release}`,
    };
  }

  function routeButton(rowRoute: UpdateRoute): VNode {
    return (
      <button
        class="action-btn whats-new-release-action"
        onClick={() => { void followUpdateRoute(rowRoute); }}
        disabled={checking}
      >
        {updateControlLabel(rowRoute, checking)}
      </button>
    );
  }

  function row(r: ChangelogRelease, openByDefault: boolean, forced?: ReleaseRowStatus) {
    const open = releaseRowIsOpen(r.version, openByDefault, toggled);
    const status = forced ?? releaseRowStatus(r.version, release, offeredVersion);
    const rowRoute = installing ? null : releaseRowRoute(status, r.version === lead, route);
    const markKind = releaseRowMark(status, rowRoute);
    return (
      <ReleaseRow
        key={r.version}
        release={r}
        mark={markKind ? markFor(markKind, r.version) : undefined}
        action={rowRoute ? routeButton(rowRoute) : null}
        open={open}
        onToggle={() => setToggled({ ...toggled, [r.version]: !open })}
      />
    );
  }

  return (
    <div class="settings-section">
      <div class="settings-section-title" data-search-anchor="whats-new:releases">
        Releases
      </div>
      {loadable.status === 'loaded' && !hasCurrent && release && (
        <div class="system-footnote">
          No release notes for {release}, the version this engine reports running.
        </div>
      )}
      {offered && (
        <div class="whats-new-list whats-new-offered">
          {row(offered, true, 'available')}
        </div>
      )}
      <LoadingFade
        showSkeleton={showSkeleton}
        skeleton={<ListSkeletonOf row={() => <ReleaseRow />} containerClass="whats-new-list" />}
      >
        {loadable.status === 'loaded' ? (
          <div class="whats-new-list">
            {releases.map((r) => row(r, r.version === openRelease))}
          </div>
        ) : null}
      </LoadingFade>
    </div>
  );
}
