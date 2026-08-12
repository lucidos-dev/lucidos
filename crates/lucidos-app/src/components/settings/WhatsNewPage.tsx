import { useEffect, useState } from 'preact/hooks';
import { changelogReleases, latestTauriAppNotes, lucidosRelease, lucidosReleaseDirty } from '../../store/store';
import { loadChangelog, markWhatsNewSeen } from '../../store/actions/whatsNew';
import { packagedUpdateVersion } from '../../store/actions/app-update';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { renderMarkdown } from '../../utils/renderMarkdown';
import { lucidosVersionTooltip } from '../../utils/lucidosVersion';
import { LoadableError } from '../shared/LoadableError';
import { LoadingFade } from '../shared/LoadingFade';
import { ListSkeletonOf, useSkeleton, SkText, SkBlock } from '../shared/Skeleton';
import { ChevronDownIcon, ChevronRightIcon } from '../shared/icons';
import type { VNode } from 'preact';
import type { ChangelogRelease } from '../../api/client';

/**
 * Which rows are open: a row the user has toggled keeps their answer, and every
 * other row falls back to "open iff it is the release you are running".
 *
 * The fallback is why this is derived per render rather than seeded into state.
 * `/health` can answer AFTER the changelog does, and a `useState(version ===
 * release)` initialiser reads its argument once: seeded that way, a client that
 * opened the panel during that window would find every release collapsed, with
 * nothing on screen explaining why.
 */
export function releaseRowIsOpen(
  version: string,
  openByDefault: boolean,
  toggled: Record<string, boolean>,
): boolean {
  return toggled[version] ?? openByDefault;
}

/** The chip a row wears, when it wears one: `Running` for the release you are
 *  on, `Available` for one the updater is offering. The parent owns the words,
 *  so the row stays a renderer. */
interface ReleaseMark {
  label: string;
  tooltip?: string;
  /** Distinguishes the offer from the installed release in CSS. */
  kind: 'running' | 'available';
}

/**
 * One release: a header that always renders, and a body that renders only when
 * the row is open.
 *
 * **Collapsed rows render no body, deliberately.** The changelog is 43 releases
 * and a couple of hundred kilobytes of markdown; rendering all of it up front
 * and hiding it with CSS would parse and lay out every word to show one release.
 */
function ReleaseRow({
  release,
  mark,
  open = false,
  onToggle,
}: {
  release?: ChangelogRelease;
  mark?: ReleaseMark;
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
    <div class={`whats-new-release${mark ? ` is-${mark.kind}` : ''}`}>
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
      {releaseNotesBody(release, open)}
    </div>
  );
}

/**
 * A release's notes, or `null` while the row is shut.
 *
 * Split out of the row so the "collapsed rows cost nothing" property is a pure
 * function a test can hold, rather than something a reader has to infer from a
 * `&&` inside a component that reads a hook. Returning null (not a hidden
 * element) is the property: 43 sections of markdown are parsed and laid out only
 * as they are asked for.
 */
export function releaseNotesBody(release: ChangelogRelease, open: boolean): VNode | null {
  if (!open) return null;
  return (
    <div
      class="markdown-content whats-new-notes"
      dangerouslySetInnerHTML={{ __html: renderMarkdown(release.notes) }}
    />
  );
}

/**
 * Split a leading `## v<version>` heading off manifest notes, keeping its date.
 *
 * **The manifest's notes are not shaped like the endpoint's.** Both come from
 * the same `CHANGELOG.md` section, but `release_notes_extract_section`
 * (scripts/lib/release_notes.sh) writes that section **header included**, while
 * the engine's parser strips it. Rendered as-is, the offered row would print its
 * version a second time as an `<h2>` inside its own body, which no other row
 * does. So the heading comes off here, at the boundary where manifest text
 * becomes a release.
 *
 * Its date comes off with it rather than being discarded, since that is the one
 * place the offered release's date exists. Same separator-blind rule the engine
 * parser uses, and for the same reason: the separator is an em dash that
 * `.claude/rules/no-em-dashes.md` forbids this source from naming.
 */
export function stripReleaseHeading(notes: string): { date: string | null; body: string } {
  const lines = notes.replace(/^\s*\n/, '').split('\n');
  const head = lines[0] ?? '';
  const rest = head.startsWith('## v') && /^\d/.test(head.slice(4)) ? head.slice(4) : null;
  if (rest === null) return { date: null, body: notes.trim() };
  const date = rest.replace(/^\S+/, '').replace(/^[^\p{L}\p{N}]+/u, '').trimEnd();
  return { date: date || null, body: lines.slice(1).join('\n').trim() };
}

/**
 * The release the updater is OFFERING, as a row for the top of the list, or
 * `null` when none is offered or its manifest carried no notes.
 *
 * This row is the only place in the app that can say what a pending update
 * contains. Its notes come from the update manifest rather than from the
 * changelog below it, because the offered version postdates the binary that
 * baked that changelog: falling back to the list would show the notes for the
 * release already running, under a heading naming the one being offered.
 */
export function offeredRelease(version: string | null, notes: string | null): ChangelogRelease | null {
  if (!version || !notes) return null;
  const { date, body } = stripReleaseHeading(notes);
  // A manifest whose notes were ONLY a heading leaves nothing to read, so it
  // degrades to no row, exactly like a manifest that carried no notes at all.
  if (!body) return null;
  return { version, date, notes: body };
}

/**
 * Settings > System > What's New: every published release, newest first, with
 * the one you are running open and marked, and any release being offered above
 * it.
 */
export function WhatsNewPage() {
  const loadable = changelogReleases.value;
  const release = lucidosRelease.value;
  const dirty = lucidosReleaseDirty.value;
  const showSkeleton = useDelayedLoading(loadable);
  const [toggled, setToggled] = useState<Record<string, boolean>>({});
  const offered = offeredRelease(packagedUpdateVersion(), latestTauriAppNotes.value);

  useEffect(() => {
    void loadChangelog();
  }, []);

  // Opening the panel is reading it, so the dot clears here rather than on any
  // particular scroll or click. Two things gate that, and both exist because the
  // dot is spent exactly once per release.
  //
  // The notes must have LOADED. Opening the panel against a disconnected engine
  // renders the error below, and marking it read there would clear the dot for a
  // release whose notes the user was never shown, permanently and silently.
  //
  // And the release must be KNOWN, which `markWhatsNewSeen` enforces itself: the
  // panel can be mounted before /health answers, and recording an unknown
  // release would spend the notice for whatever release they are actually on.
  useEffect(() => {
    if (loadable.status === 'loaded') markWhatsNewSeen(release);
  }, [release, loadable.status]);

  if (loadable.status === 'failed') {
    return <LoadableError error={loadable.error} noun="the changelog" />;
  }

  const releases = loadable.status === 'loaded' ? loadable.data : [];
  // A running release with no section of its own marks nothing, rather than
  // marking the newest and claiming something untrue. Reachable whenever RELEASE
  // is bumped ahead of its changelog entry.
  const hasCurrent = releases.some((r) => r.version === release);
  // The `*` is the same marker Settings > System > Versions uses for the same
  // fact, and the tooltip is the same sentence the Lucidos menu's version row
  // carries. One wording for one condition, in all three places.
  const runningMark: ReleaseMark = {
    kind: 'running',
    label: dirty ? 'Running *' : 'Running',
    tooltip: lucidosVersionTooltip(release, dirty),
  };

  function row(r: ChangelogRelease, mark: ReleaseMark | undefined, openByDefault: boolean) {
    const open = releaseRowIsOpen(r.version, openByDefault, toggled);
    return (
      <ReleaseRow
        key={r.version}
        release={r}
        mark={mark}
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
      {/* Outside the LoadingFade: it is the answer to "what would I get", it
          arrives from the update check rather than from this panel's fetch, and
          holding it behind the changelog's skeleton would withhold the one thing
          the user followed the update notice here to read. */}
      {offered && (
        <div class="whats-new-list whats-new-offered">
          {row(offered, { kind: 'available', label: 'Available', tooltip: `Lucidos ${offered.version} is available to install` }, true)}
        </div>
      )}
      <LoadingFade
        showSkeleton={showSkeleton}
        skeleton={<ListSkeletonOf row={() => <ReleaseRow />} containerClass="whats-new-list" />}
      >
        {loadable.status === 'loaded' ? (
          <div class="whats-new-list">
            {releases.map((r) => row(r, r.version === release ? runningMark : undefined, r.version === release))}
          </div>
        ) : null}
      </LoadingFade>
    </div>
  );
}
