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
import { renderMarkdown } from '../../utils/renderMarkdown';
import { isNewerVersion } from '../../utils/version';
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

/**
 * Which release in the history list opens by default.
 *
 * `target` is the release an update offer sent the reader here to READ, and it
 * wins: an offer's whole subject is that one release, so opening the release
 * they are RUNNING would answer a question nobody asked.
 *
 * It wins only when the list can actually show it. A packaged client's announced
 * version can name nothing on screen, which a dev workspace reproduces by
 * reporting a CalVer app build id. Falling back there leaves the panel as it is
 * on every other way in, rather than opening nothing at all.
 */
export function defaultOpenRelease(
  target: string | null,
  running: string | null,
  known: readonly ChangelogRelease[],
): string | null {
  if (target && known.some((r) => r.version === target)) return target;
  return running;
}

/** The chip a row wears, when it wears one: `Running` for the release you are
 *  on, `Available` for one the updater is offering. The parent owns the words,
 *  so the row stays a renderer. */
interface ReleaseMark {
  label: string;
  tooltip?: string;
  /** Distinguishes the offer from the installed release in CSS. */
  kind: ReleaseRowMarkKind;
}

/** Which chip a row can wear. Every status but `none`, which marks nothing. */
export type ReleaseRowMarkKind = Exclude<ReleaseRowStatus, 'none'>;

/** What a release row IS, relative to the one running and the one on offer.
 *
 *  `newer` is the case this panel could not express before. The published
 *  changelog can list a release the update check has not offered, because the
 *  gateway polls periodically and the two sources are independent. */
export type ReleaseRowStatus = 'available' | 'newer' | 'running' | 'none';

/**
 * Classify one row. Pure, and the whole of the rule.
 *
 * The offer wins over `running`, though the two can never name one release: an
 * offer is newer than what is running by construction, on both the gateway path
 * and the client one.
 *
 * A null `running` is the window before `/health` answers. It marks nothing
 * rather than guessing, which is the same call {@link defaultOpenRelease}
 * makes.
 */
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

/**
 * The row that carries the panel's update control, or `null` when the reader is
 * on the newest release the panel knows.
 *
 * A route is a GLOBAL answer, so a control on every ahead row would repeat
 * itself down the list and say nothing new each time.
 *
 * **The OFFER wins wherever it sits, and only then does the newest ahead row.**
 * The two sources move independently. The published changelog reaches the
 * mirror before the update-check origin announces the same release, and the
 * origin can stage a rollout deliberately (ADR 0108). So the list can carry a
 * release the updater will not install. A control above the offer would then
 * install a version its own row does not name.
 *
 * The synthesized offered row is not passed here. It sits above this list and
 * the caller takes it as the lead, since a release absent from the list cannot
 * be found in it.
 */
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

/**
 * What a row lets the reader DO about its release, or `null` for nothing.
 *
 * **A row the panel marks as ahead of you always gets a route** (ADR 0142).
 * That is the whole point of this function: the panel may not say a release is
 * newer and then leave the reader with no way to get it. `route` is
 * `updateRoute()`, so every install shape gets the answer that fits it.
 *
 * Only the LEAD row carries it, for the reason {@link leadAheadVersion} gives.
 */
export function releaseRowRoute(
  status: ReleaseRowStatus,
  isLead: boolean,
  route: UpdateRoute,
): UpdateRoute | null {
  if (!isLead) return null;
  return status === 'available' || status === 'newer' ? route : null;
}

/**
 * Which chip a row wears, or `null` for none.
 *
 * An Update button supersedes the chip: it already says the release is
 * available, so an `Available` chip beside it states one fact twice.
 *
 * Every other route keeps the chip, because it states a different fact. "Newer"
 * says this release is published and you are not on it. "Check for Updates" and
 * "How to update" say what to do next, and neither implies the first.
 */
export function releaseRowMark(
  status: ReleaseRowStatus,
  route: UpdateRoute | null,
): ReleaseRowMarkKind | null {
  if (status === 'none' || route === 'install') return null;
  return status;
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
  action,
  open = false,
  onToggle,
}: {
  release?: ChangelogRelease;
  mark?: ReleaseMark;
  /** The control this row offers, already built by the parent. A rendered node
   *  rather than a descriptor, so the row stays a renderer and the wording
   *  lives beside the handler that acts on it. */
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
    // `data-release` is how a deep link finds its row: the panel brings the
    // release an update offer named into view once that row exists.
    <div class={`whats-new-release${mark ? ` is-${mark.kind}` : ''}`} data-release={release.version}>
      {/* The action is a SIBLING of the toggle, never inside it: the toggle is
          itself a button, and a button nested in one is invalid markup that no
          browser routes sensibly. */}
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
 * Split a leading `## v<version>` heading off manifest notes, keeping the
 * version and the date it named.
 *
 * **The manifest's notes are not shaped like the endpoint's.** Both come from
 * the same `CHANGELOG.md` section, but `release_notes_extract_section`
 * (scripts/lib/release_notes.sh) writes that section **header included**, while
 * the engine's parser strips it. Rendered as-is, the offered row would print its
 * version a second time as an `<h2>` inside its own body, which no other row
 * does. So the heading comes off here, at the boundary where manifest text
 * becomes a release.
 *
 * Its version and date come off with it rather than being discarded. The
 * heading is the one place the offered release NAMES itself, which is what
 * {@link offeredRelease} identifies the row by. Same separator-blind rule the
 * engine parser uses, and for the same reason: the separator is an em dash that
 * `.claude/rules/no-em-dashes.md` forbids this source from naming.
 */
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

/**
 * The release the updater is OFFERING, as a row for the top of the list, or
 * `null` when there is nothing to show.
 *
 * This row is the only place in the app that can say what a pending update
 * contains. Its notes come from the update manifest rather than from the
 * changelog below it, because the offered version can postdate both the baked
 * changelog and the published one: falling back to the list would show the
 * notes for the release already running, under a heading naming another.
 *
 * **The version comes from the notes' own heading, not from `version`.** That
 * argument is derived from `latestTauriAppVersion`, which the health poll
 * overwrites every few seconds with the engine's `latest_tauri_app_version`. On
 * a dev workspace that field is a CalVer app build id, not a release. So the row
 * headed itself with a build id, or vanished while the offer toast beside it
 * still named the release. A heading cannot disagree with the notes under it.
 * `version` stays as the fallback for a hand-cut manifest carrying no heading.
 *
 * `known` is the list this row sits above. A release already in it is not news,
 * and rendering it anyway would show one version twice.
 */
export function offeredRelease(
  version: string | null,
  notes: string | null,
  known: readonly ChangelogRelease[] = [],
): ChangelogRelease | null {
  if (!notes) return null;
  const { version: named, date, body } = stripReleaseHeading(notes);
  const offered = named ?? version;
  // A manifest whose notes were ONLY a heading leaves nothing to read, so it
  // degrades to no row, exactly like a manifest that carried no notes at all.
  if (!offered || !body) return null;
  if (known.some((r) => r.version === offered)) return null;
  return { version: offered, date, notes: body };
}

/**
 * Settings > System > What's New: every published release, newest first, with
 * the one you are running open and marked, and any release being offered above
 * it. Arriving from an update offer opens the release that offer announced
 * instead.
 *
 * **A release marked ahead of you always carries the route to it.** The panel
 * lists the PUBLISHED changelog and the updater polls on its own schedule, so
 * it knows about releases no offer has named. It used to mark those and stop
 * there, leaving an update on screen and out of reach.
 */
export function WhatsNewPage() {
  const loadable = changelogReleases.value;
  const release = lucidosRelease.value;
  const dirty = lucidosReleaseDirty.value;
  const showSkeleton = useDelayedLoading(loadable);
  const [toggled, setToggled] = useState<Record<string, boolean>>({});
  const [target, setTarget] = useState<string | null>(null);

  useEffect(() => {
    void loadChangelog();
  }, []);

  // The release an update offer sent the reader here to READ, taken into this
  // visit's own state and cleared. It is a navigation parameter. Held in the
  // signal it would re-open the same row on a visit made for another reason,
  // and it would fight the reader's own toggles.
  useEffect(() => {
    const requested = whatsNewTargetRelease.value;
    if (!requested) return;
    whatsNewTargetRelease.value = null;
    setTarget(requested);
  }, [whatsNewTargetRelease.value]);

  // Bring that release into view once its row exists.
  //
  // `nearest` scrolls NOTHING when the row is already visible, which is the
  // ordinary case: the deep link dropped this view's remembered scroll, so the
  // panel opens at the top and the announced release is the first row. It earns
  // its place in the one case no remount covers, the panel already open and
  // scrolled somewhere else when the offer is tapped.
  //
  // Escaped, because a version is not a literal this file wrote: it arrives from
  // an update manifest. A stray quote in one would make `querySelector` throw
  // out of an effect, taking the panel with it.
  useEffect(() => {
    if (!target || loadable.status !== 'loaded') return;
    const row = document.querySelector(`[data-release="${CSS.escape(target)}"]`);
    row?.scrollIntoView({ block: 'nearest' });
  }, [target, loadable.status]);

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
  const offeredVersion = packagedUpdateVersion();
  // Derived from the list, not just above it: the list can now carry a release
  // newer than the running one, the offered one included.
  const offered = offeredRelease(offeredVersion, latestTauriAppNotes.value, releases);
  // What this session can do about a release ahead of the running one. One
  // answer for the whole panel, so the control it renders is the same one
  // Settings, System would render.
  const route = updateRoute();
  // The row that carries it. The offered row wins when it exists, since it sits
  // above the list and nothing published can postdate it.
  const lead = offered?.version ?? leadAheadVersion(releases, release, offeredVersion);
  // An install already under way owns every update control: the progress dialog
  // narrates it, and a row offering to start another would be a lie.
  const installing = appUpdateProgress.value !== null;
  // A check the USER started, so the control says so and refuses a second. The
  // same signal Settings, System reads, for the same reason.
  const checking = appUpdateCheckInFlight.value;
  // A running release with no section of its own marks nothing, rather than
  // marking the newest and claiming something untrue. Reachable whenever RELEASE
  // is bumped ahead of its changelog entry.
  const hasCurrent = releases.some((r) => r.version === release);
  // Which row opens by itself. The Available row above is not in this list and
  // keeps its own unconditional open: it IS the announced release whenever it
  // exists.
  const openRelease = defaultOpenRelease(target, release, releases);
  // The `*` is the same marker Settings > System > Versions uses for the same
  // fact, and the tooltip is the same sentence the Lucidos menu's version row
  // carries. One wording for one condition, in all three places.
  const runningMark: ReleaseMark = {
    kind: 'running',
    label: dirty ? 'Running *' : 'Running',
    tooltip: lucidosVersionTooltip(release, dirty),
  };

  /** The chip a kind wears. `running` is the only one needing state from
   *  outside the row, which is why it is built above rather than here. */
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

  /** The one control a row can offer. Its wording and its click are both
   *  Settings → System's, so the two surfaces cannot disagree about what taking
   *  the update does.
   *
   *  That includes reporting a check in flight and refusing a second one. The
   *  verdict arrives as an unkeyed toast, so a button that stayed live through
   *  the round trip stacked one copy per press. */
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
    // The offered row FORCES its status. Its version comes from the manifest's
    // own heading, which can name something `packagedUpdateVersion` does not,
    // and that row is the offer whatever the two say. See {@link offeredRelease}.
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
      {/* Outside the LoadingFade: it is the answer to "what would I get", it
          arrives from the update check rather than from this panel's fetch, and
          holding it behind the changelog's skeleton would withhold the one thing
          the user followed the update notice here to read. */}
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
