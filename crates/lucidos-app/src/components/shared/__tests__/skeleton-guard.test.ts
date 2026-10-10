import { describe, it, expect } from 'vitest';
import {
  exemptionProblems,
  rawSourceOf,
  scanSources,
  stripComments,
  type Exemption,
} from './loading-guard-scan';

/**
 * Loading states draw a skeleton built from the real component, never a bare
 * loader (`.claude/rules/frontend.md` § "Async Data Loading"). Source scans,
 * default-deny:
 *
 *  - A: the retired generic-bar `ListSkeleton` stays gone (only `ListSkeletonOf`).
 *  - B: every `<LoadingFade>` is paired with a self-skeletonizing skeleton.
 *  - C: no bare loader: no `loading-spinner` class and no "Loading…" text.
 *    Every component that consumes a `Loadable` draws a skeleton, unless an
 *    exemption names the rule that lets it skip one.
 */

const RETIRED_GENERIC = /\bListSkeleton\b/;

/** A self-skeletonizing form: the provider, the list helper, a capitalised
 *  `*Skeleton` component, or a row reading `useSkeleton()`. Capitalised on
 *  purpose: `LoadingFade`'s own `showSkeleton` prop must not count. */
const APPROVED_SKELETON = /ListSkeletonOf|SkeletonProvider|\b[A-Z]\w*Skeleton\b|folderTreeSkeletonRow|useSkeleton\(\)/;

function drawsSkeleton(code: string): boolean {
  return APPROVED_SKELETON.test(code);
}

/** Branches on a Loadable's status, or gates a loader on one. */
function consumesLoadable(code: string): boolean {
  return /\.status\s*(===|!==)\s*'(loading|loaded|not-loaded)'|\buseDelayedLoading\(|\buseLoadableFetch\b/.test(code);
}

function hasBareSpinner(code: string): boolean {
  return /\bloading-spinner\b/.test(code);
}

function loadingLiterals(code: string): string[] {
  return code.match(/Loading(?: [a-z]+)*(?:…|\.\.\.)/g) ?? [];
}

/** Consumers that may skip a skeleton, each with the rule that says so. */
const CONSUMER_EXEMPT: Record<string, Exemption> = {
  'components/chat/chat-exchange-parts.tsx': { tag: 'single-value', why: 'a change card holds its headline line empty until it loads' },
  'components/chat/ComposeDestinationRow.tsx': { tag: 'structure-first', why: 'destination dropdown; the lists only fill its options' },
  'components/chat/FrontendPreviewSection.tsx': { tag: 'structure-first', why: 'control menu section; dims its button until the slot is read' },
  'components/chat/LucidosControlMenu.tsx': { tag: 'structure-first', why: 'anchored menu; only kicks off the model list read' },
  'components/chat/MessageRoutePanel.tsx': { tag: 'no-visual', why: 'label lookup that falls back to the raw id' },
  'components/chat/MicrophonePicker.tsx': { tag: 'structure-first', why: 'anchored popover drawn from the first frame' },
  'components/layout/Drawer.tsx': { tag: 'no-visual', why: 'derives the pinned set; draws nothing itself' },
  'components/picker/NetworkAccessPopover.tsx': { tag: 'structure-first', why: 'the canonical structure-first popover' },
  'components/plugins/CatalogScanFailure.tsx': { tag: 'single-value', why: 'a failure notice beside a list with its own skeleton' },
  'components/settings/AddDeviceSection.tsx': { tag: 'working-state', why: 'mints a pairing code on a button press' },
  'components/settings/DebuggingSection.tsx': { tag: 'structure-first', why: 'LoadableToggle holds the switch and defers its value' },
  'components/settings/JudgmentModelRow.tsx': { tag: 'structure-first', why: 'real picker row; defers which choices it offers' },
  'components/settings/MemoryModuleSection.tsx': { tag: 'structure-first', why: 'real segmented control; presses nothing until preferences load' },
  'components/settings/MemoryPage.tsx': { tag: 'structure-first', why: 'real sections; the inspector stays until preferences say Tree' },
  'components/settings/TreeBackfillStatus.tsx': { tag: 'structure-first', why: 'real progress row; the bar track draws at once, values wait' },
  'components/settings/ProviderBlock.tsx': { tag: 'structure-first', why: 'switch drawn at once; config rows stay shut until known' },
  'components/settings/SystemOneProviderFrame.tsx': { tag: 'structure-first', why: 'real controls; defer their values' },
  'components/settings/CustomSystemOneSettings.tsx': { tag: 'structure-first', why: 'real controls; defer their values' },
  'components/apps/ReusableWidgetsSection.tsx': { tag: 'single-value', why: 'a group under the apps list, which draws its own skeleton' },
  'components/widgets/WidgetCard.tsx': { tag: 'single-value', why: "a widget's bar holds its name empty until the thread's widgets load" },
  'components/widgets/HomeWidgetsMenu.tsx': { tag: 'structure-first', why: 'anchored popover drawn from the first frame under its label' },
  'components/widgets/WidgetShelf.tsx': { tag: 'single-value', why: 'the title row stays as it was until its widget chips land' },
  'components/widgets/WidgetWindows.tsx': { tag: 'no-visual', why: "a widget window draws once Home's widgets name it, and nothing before" },
  'components/shared/AppIcon.tsx': { tag: 'single-value', why: 'an icon slot shows the monogram tile until the apps list lands' },
};

/** "Loading…" literals that may stay, keyed by file. */
const LITERAL_EXEMPT: Record<string, Exemption & { literal: string }> = {
  'components/chat/composeDestinationOptions.ts': {
    tag: 'structure-first', literal: 'Loading…', why: 'a disabled option inside the destination dropdown',
  },
  'components/triggers/TriggerDetails.tsx': {
    tag: 'structure-first', literal: 'Loading event types...', why: 'the placeholder option of the event-type picker',
  },
};

const components = scanSources('components', ['.ts', '.tsx']);
const tsx = components.filter((f) => f.path.endsWith('.tsx'));

describe('skeleton guard scanners', () => {
  it('ignores comments that name a banned form', () => {
    expect(loadingLiterals(stripComments('// no "Loading…" here\nconst a = 1;'))).toEqual([]);
    expect(hasBareSpinner(stripComments('/* was <div class="loading-spinner" /> */'))).toBe(false);
  });

  it('spots every bare loader shape', () => {
    expect(hasBareSpinner('<div class="loading-spinner" />')).toBe(true);
    expect(loadingLiterals('<div>Loading...</div> <p>Loading more…</p>')).toEqual(['Loading...', 'Loading more…']);
    expect(consumesLoadable("if (x.status !== 'loaded') return null;")).toBe(true);
    expect(consumesLoadable('const show = useDelayedLoading(items.value);')).toBe(true);
  });

  it('does not count the showSkeleton prop of LoadingFade as a skeleton', () => {
    expect(drawsSkeleton('<LoadingFade showSkeleton={s} skeleton={<div />}>')).toBe(false);
    expect(drawsSkeleton('<LoadingFade showSkeleton={s} skeleton={<ListSkeletonOf row={r} />}>')).toBe(true);
    expect(drawsSkeleton('<DropdownSkeleton w="6rem" />')).toBe(true);
  });

  it('reports a bad exemption list', () => {
    const offenders = new Set(['a.tsx', 'b.tsx']);
    const raw = (p: string) => (p === 'gone.tsx' ? null : p === 'b.tsx' ? 'no comment' : 'x');
    expect(exemptionProblems({
      'a.tsx': { tag: 'nope' as never, why: '' },
      'gone.tsx': { tag: 'single-value', why: '' },
      'c.tsx': { tag: 'single-value', why: '' },
      'b.tsx': { tag: 'best-effort', why: '' },
    }, offenders, raw)).toHaveLength(4);
  });
});

describe('skeleton convention guard', () => {
  it('does not reintroduce the retired generic-bar list skeleton (only ListSkeletonOf)', () => {
    const offenders = scanSources('.', ['.ts', '.tsx'])
      .filter((f) => RETIRED_GENERIC.test(f.code))
      .map((f) => f.path);
    expect(offenders, 'use ListSkeletonOf (self-skeletonizing), not the retired generic list skeleton').toEqual([]);
  });

  it('every <LoadingFade> is paired with a self-skeletonizing skeleton', () => {
    const offenders = tsx
      .filter((f) => f.code.includes('<LoadingFade') && !drawsSkeleton(f.code))
      .map((f) => f.path);
    expect(offenders, 'pass a self-skeletonizing skeleton (ListSkeletonOf, SkeletonProvider, a *Skeleton component)').toEqual([]);
  });

  it('draws no bare loader', () => {
    const consumers = new Set(tsx.filter((f) => consumesLoadable(f.code) && !drawsSkeleton(f.code)).map((f) => f.path));
    const spinners = new Set(components.filter((f) => hasBareSpinner(f.code)).map((f) => f.path));
    const literalHolders = new Set(components.filter((f) => loadingLiterals(f.code).length > 0).map((f) => f.path));
    const literals = new Set(components
      .filter((f) => loadingLiterals(f.code).some((l) => l !== LITERAL_EXEMPT[f.path]?.literal))
      .map((f) => f.path));
    const all = new Set([...consumers, ...spinners, ...literals]);

    // A spinner or an unexempt literal is never excused; a consumer may be.
    const excused = (p: string) => !!CONSUMER_EXEMPT[p] && !spinners.has(p) && !literals.has(p);
    expect([...all].filter((p) => !excused(p)), 'draw a skeleton from the real component (frontend.md § Async Data Loading)').toEqual([]);
    expect([
      ...exemptionProblems(CONSUMER_EXEMPT, consumers, rawSourceOf),
      ...exemptionProblems(LITERAL_EXEMPT, literalHolders, rawSourceOf),
    ]).toEqual([]);
  });
});
