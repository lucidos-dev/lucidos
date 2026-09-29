import { useEffect, useLayoutEffect, useRef, useState } from 'preact/hooks';
import {
  DEFAULT_THEME_ID,
  FOLLOW_THEME,
  fontFeaturesFor,
  fontStackFor,
  resolveFontKey,
  type ThemeModeMaps,
  type ResolvedThemeMode,
} from '@lucidos/appearance';
import { registerWorkspaceFont } from '@lucidos/font-faces';
import { THEME_FAMILIES, dataMountUrl, type Theme, type ThemeFamily } from '../../api/client';
import { themeGallery, loadThemeGallery, pickTheme } from '../../store/actions/themes';
import { currentThemeId, paintedThemeMode } from '../../store/actions/preferences';
import { preferences } from '../../store/store';
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { isReducedMotion } from '../../utils/motion';
import { viewportIsMobile } from '../../utils/viewport';
import { ChevronLeftIcon, ChevronRightIcon } from '../shared/icons';
import { LoadingFade } from '../shared/LoadingFade';
import { LoadableError } from '../shared/LoadableError';
import { ListSkeletonOf, SkBlock, SkText, useSkeleton } from '../shared/Skeleton';

function modesLabel(theme: Theme): string | null {
  if (theme.modes.length !== 1) return null;
  return theme.modes[0] === 'light' ? 'Light only' : 'Dark only';
}

const FAMILY_NAMES: Record<ThemeFamily, string> = {
  blue: 'Cool',
  violet: 'Violet',
  warm: 'Warm',
  neutral: 'Neutral',
};

export interface ThemeFamilyGroup {
  name: string;
  themes: Theme[];
}

/** Splits themes into one section per family, in family order, keeping each
 *  theme's listed order. Every other theme, with no family or one this bundle
 *  does not know, comes last as "Other", so no theme drops out of the picker. */
export function groupThemesByFamily(themes: Theme[]): ThemeFamilyGroup[] {
  const groups: ThemeFamilyGroup[] = THEME_FAMILIES.map(family => ({
    name: FAMILY_NAMES[family],
    themes: themes.filter(theme => theme.family === family),
  }));
  const known = new Set<string | undefined>(THEME_FAMILIES);
  groups.push({ name: 'Other', themes: themes.filter(theme => !known.has(theme.family)) });
  return groups.filter(group => group.themes.length > 0);
}

/** The font the theme would paint if the user followed it, as the two custom
 *  properties the card's name reads. */
function themeFontStyle(theme: Theme): Record<string, string> {
  const known = theme.resolved.workspace_fonts;
  const font = resolveFontKey(FOLLOW_THEME, theme.resolved.fonts, known);
  return {
    '--font-ui': fontStackFor(font, known),
    '--font-features-text': fontFeaturesFor(font, known).text,
  };
}

/** Which ends of the strip already show. */
interface StripEnds {
  atStart: boolean;
  atEnd: boolean;
}

/** The narrowest a card may get before the strip drops a column. */
const THEME_CARD_MIN_REM = 9;

/** A phone card previews a portrait phone screen, so it is narrower than a
 *  desktop card. */
const PEEK_CARD_REM = 7;

/** On a phone, the centred card's widest share of the strip. Even a narrow
 *  strip then leaves its neighbours room to peek in at both sides. */
const PEEK_CARD_MAX_SHARE = 0.62;

/** Marks a phone's strip, the skeleton's included. */
const PEEK_CLASS = 'theme-carousel-peek';

function peekCardWidth(el: HTMLElement): number {
  const rootPx = parseFloat(getComputedStyle(document.documentElement).fontSize);
  return Math.min(PEEK_CARD_REM * rootPx, el.clientWidth * PEEK_CARD_MAX_SHARE);
}

interface StripGeometry {
  columns: number;
  /** How far one page moves the strip, its gap included. */
  page: number;
  /** How far left of a page's start the strip stops, so a phone's card sits
   *  centred. The browser clamps the first and last stop to the strip's ends,
   *  so the end cards sit at the edges and leave no side empty. */
  offset: number;
}

/** The strip's layout from its own box. A PEEK_CLASS strip (a phone) centres
 *  one card with its neighbours peeking in. Any other fits as many whole
 *  columns as it can, and a page is all of them. */
function stripGeometry(el: HTMLElement): StripGeometry {
  const style = getComputedStyle(el);
  const gap = parseFloat(style.columnGap) || 0;
  if (el.classList.contains(PEEK_CLASS)) {
    const card = peekCardWidth(el);
    const offset = (el.clientWidth - card) / 2 - parseFloat(style.paddingLeft);
    return { columns: 1, page: card + gap, offset };
  }
  const content = el.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
  const rootPx = parseFloat(getComputedStyle(document.documentElement).fontSize);
  const columns = Math.max(1, Math.floor((content + gap) / (THEME_CARD_MIN_REM * rootPx + gap)));
  const page = content + gap;
  return { columns, page, offset: 0 };
}

/** Sizes the columns to the strip: whole columns filling it exactly, or on a
 *  phone a card narrow enough for its neighbours to peek in. */
function fitColumns(el: HTMLElement): void {
  if (el.classList.contains(PEEK_CLASS)) {
    el.style.setProperty('--theme-card-width', `${peekCardWidth(el)}px`);
    return;
  }
  el.style.setProperty('--theme-columns', String(stripGeometry(el).columns));
}

interface FamilyPlacement {
  group: ThemeFamilyGroup;
  /** First grid column, 1-based. */
  column: number;
  span: number;
}

/** Lays families side by side, each starting on a fresh column and filling
 *  `rows` rows, so a page boundary always falls between whole cards. */
function placeFamilies(groups: ThemeFamilyGroup[], rows: number): FamilyPlacement[] {
  let column = 1;
  return groups.map(group => {
    const span = Math.ceil(group.themes.length / rows);
    const placed = { group, column, span };
    column += span;
    return placed;
  });
}

function stripEnds(el: HTMLElement): StripEnds {
  return { atStart: el.scrollLeft <= 1, atEnd: el.scrollLeft + el.clientWidth >= el.scrollWidth - 1 };
}

/**
 * Every theme as a radio card in a strip that scrolls sideways, a page of
 * whole columns at a time. A chevron floats over each side and pages it, and
 * the cards fade out at a side with more past it. The strip is two cards
 * tall, or one when every card shown fits in a single row. On a phone it is
 * one row that centres a card at a time, with its neighbours peeking in. Above it, filter chips pick All or one family. All
 * names each family above its first card.
 *
 * A pick paints at once and the strip stays, so themes can be compared.
 * Each card previews itself: it sets every catalog token at its default, then
 * the theme's own resolved map, as custom properties on the preview element.
 * So the preview resolves each `var()` against that theme alone and never
 * inherits the page's active theme.
 */
export function ThemePicker() {
  useEffect(() => {
    if (themeGallery.value.status === 'not-loaded') void loadThemeGallery();
  }, []);
  const gallery = themeGallery.value;
  const showSkeleton = useDelayedLoading(gallery);
  const strip = useRef<HTMLDivElement>(null);
  const phone = viewportIsMobile.value;
  const maxRows = phone ? 1 : 2;
  // How many whole columns the strip fits, 0 until it is measured.
  const [columns, setColumns] = useState(0);
  // The family chip picked, or null for All.
  const [filter, setFilter] = useState<string | null>(null);
  const groups = gallery.status === 'loaded' ? groupThemesByFamily(gallery.data.themes) : [];
  // A family a reload dropped falls back to All rather than an empty strip.
  const picked = groups.some(group => group.name === filter) ? filter : null;
  const shown = picked === null ? groups : groups.filter(group => group.name === picked);
  const cardCount = shown.reduce((count, group) => count + group.themes.length, 0);
  const rows = cardCount <= columns ? 1 : maxRows;
  // All names each family above its first card, on a row of its own.
  const labelRows = picked === null ? 1 : 0;
  const families = placeFamilies(shown, rows);
  const [ends, setEnds] = useState<StripEnds>({ atStart: true, atEnd: true });
  // Runs on every scroll, so it keeps the old state when neither end moved:
  // a fresh object would re-render every card each frame.
  const measure = () => {
    if (!strip.current) return;
    const next = stripEnds(strip.current);
    setEnds(prev => (prev.atStart === next.atStart && prev.atEnd === next.atEnd ? prev : next));
  };
  const fit = () => {
    if (!strip.current) return;
    fitColumns(strip.current);
    setColumns(stripGeometry(strip.current).columns);
  };
  const loaded = gallery.status === 'loaded';
  useLayoutEffect(() => {
    fit();
    measure();
  }, [loaded, phone]);
  // Opens on the page holding the active theme, once the gallery and the
  // preference naming the active theme are both in. Earlier would open on the
  // default, and a later pick must not move the strip under the reader.
  const ready = loaded && preferences.value.status === 'loaded';
  useLayoutEffect(() => {
    if (!ready) return;
    const scroller = strip.current;
    const card = scroller?.querySelector<HTMLElement>('.theme-card[aria-checked="true"]');
    if (!scroller || !card) return;
    const { columns, page, offset } = stripGeometry(scroller);
    const column = parseInt(card.style.gridColumnStart, 10) - 1;
    scroller.scrollLeft = Math.max(0, Math.floor(column / columns) * page - offset);
    measure();
  }, [ready]);
  // A wider or narrower column changes how many cards fit, and moves the ends
  // without a scroll event.
  useEffect(() => {
    const scroller = strip.current;
    if (!scroller || typeof ResizeObserver === 'undefined') return;
    const observer = new ResizeObserver(() => {
      fit();
      measure();
    });
    observer.observe(scroller);
    return () => observer.disconnect();
  }, [loaded]);
  const scrollTo = (left: number) => {
    strip.current?.scrollTo({ left: Math.max(0, left), behavior: isReducedMotion() ? 'auto' : 'smooth' });
  };
  // Moves one page on, so every stop shows whole pages. Whole columns round
  // toward the direction of travel: after a clamp at an end or a swipe the
  // strip rests between stops, and rounding to the nearest would skip cards.
  // A phone steps on from the card nearest the centre instead: at an end that
  // is the second card, so a press centres the third.
  const pageBy = (direction: 1 | -1) => {
    const scroller = strip.current;
    if (!scroller) return;
    const { page, offset } = stripGeometry(scroller);
    const at = (scroller.scrollLeft + offset) / page;
    const from = offset !== 0 ? Math.round(at) : direction > 0 ? Math.floor(at + 0.01) : Math.ceil(at - 0.01);
    scrollTo((from + direction) * page - offset);
  };
  // A new filter or row count lays the cards out anew, so the ends move with
  // no scroll.
  useLayoutEffect(measure, [picked, rows]);
  const pickFilter = (name: string | null) => {
    if (strip.current) strip.current.scrollLeft = 0;
    setFilter(name);
  };
  if (gallery.status === 'failed') return <LoadableError noun="themes" error={gallery.error} />;

  const mode = paintedThemeMode.value;
  const stripClass = phone ? `theme-carousel ${PEEK_CLASS}` : 'theme-carousel';
  const active = currentThemeId();
  const themes = gallery.status === 'loaded' ? gallery.data.themes : null;
  // A preference naming a theme that is gone paints the default
  // (refreshActiveTheme), so the picker checks the default too.
  const painted = themes?.find(theme => theme.id === active) ?? themes?.find(theme => theme.id === DEFAULT_THEME_ID);
  return (
    <LoadingFade
      showSkeleton={showSkeleton}
      skeleton={
        // The loaded frame's rows: a chip line, the names row, then the cards.
        <div class="theme-carousel-frame">
          <div class="pill-bar" aria-hidden="true">
            <span class="pill-bar-btn">{'\u00a0'}</span>
          </div>
          <ListSkeletonOf
            containerClass={stripClass}
            count={6}
            row={i => (
              <>
                {i === 0 && <span class="theme-family-name">{'\u00a0'}</span>}
                <ThemeCard mode={mode} column={1 + Math.floor(i / maxRows)} row={2 + (i % maxRows)} />
              </>
            )}
          />
        </div>
      }
    >
      {gallery.status === 'loaded' && (
        <div class="theme-carousel-frame">
          <div class="pill-bar" role="group" aria-label="Theme family">
            {[null, ...groups.map(group => group.name)].map(name => (
              <button
                key={name ?? 'all'}
                type="button"
                class={`pill-bar-btn${picked === name ? ' active' : ''}`}
                aria-pressed={picked === name}
                onClick={() => pickFilter(name)}
              >
                {name ?? 'All'}
              </button>
            ))}
          </div>
          <div class="theme-carousel-body">
            <button type="button" class="icon-btn theme-carousel-step theme-carousel-prev" aria-label="Previous themes" disabled={ends.atStart} onClick={() => pageBy(-1)}>
              <ChevronLeftIcon />
            </button>
            <div
              ref={strip}
              class={stripClass}
              role="radiogroup"
              aria-label="Theme"
              data-more-before={ends.atStart ? undefined : ''}
              data-more-after={ends.atEnd ? undefined : ''}
              onScroll={measure}
            >
              {families.map(({ group, column, span }) => (
                <div key={group.name} class="theme-family" role="group" aria-label={group.name}>
                  {labelRows > 0 && (
                    <span class="theme-family-name" aria-hidden="true" style={{ gridColumn: `${column} / span ${span}` }}>
                      {group.name}
                    </span>
                  )}
                  {group.themes.map((theme, i) => {
                    const selected = theme.id === painted?.id;
                    // The active theme needs no pick, unless it lacks this mode:
                    // then pickTheme offers the mode switch.
                    const settled = selected && theme.modes.includes(mode);
                    return (
                      <ThemeCard
                        key={theme.id}
                        theme={theme}
                        defaults={gallery.data.defaults}
                        mode={mode}
                        selected={selected}
                        column={column + Math.floor(i / rows)}
                        row={labelRows + 1 + (i % rows)}
                        onClick={settled ? undefined : () => void pickTheme(theme)}
                      />
                    );
                  })}
                </div>
              ))}
            </div>
            <button type="button" class="icon-btn theme-carousel-step theme-carousel-next" aria-label="Next themes" disabled={ends.atEnd} onClick={() => pageBy(1)}>
              <ChevronRightIcon />
            </button>
          </div>
        </div>
      )}
    </LoadingFade>
  );
}

/** One theme's radio in the carousel, at its grid cell. */
function ThemeCard({ theme, defaults, mode, selected = false, column, row, onClick }: {
  theme?: Theme;
  defaults?: ThemeModeMaps;
  mode: ResolvedThemeMode;
  selected?: boolean;
  column: number;
  row: number;
  onClick?: () => void;
}) {
  const sk = useSkeleton();
  useEffect(() => {
    for (const font of theme?.resolved.workspace_fonts ?? []) registerWorkspaceFont(font, dataMountUrl);
  }, [theme]);
  // A single-mode theme previews the mode it has, so Paper never shows as navy.
  const previewMode = theme?.modes.length === 1 ? theme.modes[0] : mode;
  // The gallery sanitised these maps when it loaded (store/actions/themes.ts).
  const tokens = theme ? theme.resolved[previewMode] : {};
  const style = defaults ? { ...defaults[previewMode], ...tokens } : undefined;
  const label = theme ? modesLabel(theme) : null;
  const tooltip = theme ? [theme.description, theme.credit].filter(Boolean).join(' ') : undefined;

  return (
    <button
      type="button"
      role="radio"
      aria-checked={selected}
      class={`theme-card${selected ? ' selected' : ''}`}
      style={{ gridColumn: String(column), gridRow: String(row) }}
      data-tooltip={tooltip || undefined}
      disabled={sk}
      onClick={onClick}
    >
      {/* The preview's height in themes.css: portrait on a phone. */}
      <SkBlock w="100%" h={viewportIsMobile.value ? '11rem' : '5rem'} round>
        <div class="theme-preview" style={style} aria-hidden="true">
          <div class="theme-preview-header">
            <span class="theme-preview-dot" />
            <span class="theme-preview-dot" />
            <span class="theme-preview-focus" />
          </div>
          <div class="theme-preview-body">
            <div class="theme-preview-panel">
              <span class="theme-preview-line theme-preview-line-strong" />
              <span class="theme-preview-line" />
              <span class="theme-preview-line theme-preview-line-muted" />
            </div>
            <div class="theme-preview-side">
              <span class="theme-preview-button" />
              <span class="theme-preview-status">
                <i class="theme-preview-green" />
                <i class="theme-preview-yellow" />
                <i class="theme-preview-red" />
              </span>
            </div>
          </div>
        </div>
      </SkBlock>
      <span class="theme-card-meta" style={theme ? themeFontStyle(theme) : undefined}>
        <SkText class="theme-card-name" w="50%">{theme?.name}</SkText>
        {label && <span class="theme-card-modes">{label}</span>}
      </span>
    </button>
  );
}
