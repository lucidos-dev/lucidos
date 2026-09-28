import { useEffect, useRef, useState } from 'preact/hooks';
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
import { useDelayedLoading } from '../../hooks/useDelayedLoading';
import { Disclosure } from '../shared/Disclosure';
import { ChevronRightIcon } from '../shared/icons';
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

/**
 * The active theme's card alone, unfolding into every theme grouped by family.
 * Each card previews itself: it sets every catalog token at its default, then
 * the theme's own resolved map, as custom properties on the preview element. So
 * the preview resolves each `var()` against that theme alone and never inherits
 * the page's active theme.
 */
export function ThemePicker() {
  useEffect(() => {
    if (themeGallery.value.status === 'not-loaded') void loadThemeGallery();
  }, []);
  // Shut on arrival, page-local like every other settings disclosure: the
  // grid is a dozen previews, and most visits to Appearance are not for it.
  const [open, setOpen] = useState(false);
  const root = useRef<HTMLDivElement>(null);
  // Whether focus was inside when the grid toggled, so it follows the swap
  // rather than dropping to the body with the card it was on.
  const refocus = useRef(false);
  const toggleOpen = (next: boolean) => {
    refocus.current = root.current?.contains(document.activeElement) ?? false;
    setOpen(next);
  };
  useEffect(() => {
    if (!refocus.current) return;
    refocus.current = false;
    const target = open ? '.theme-card[aria-checked="true"]' : '.theme-toggle';
    root.current?.querySelector<HTMLElement>(target)?.focus({ preventScroll: true });
  }, [open]);
  useEffect(() => {
    if (!open) return;
    // Any click but one on a theme folds the grid. A theme card answers its own
    // click: another theme is picked, and the active one folds or offers its
    // mode switch. A click while an overlay is open belongs to that overlay,
    // such as the mode-switch confirm a pick raises. Capture phase, so a
    // control that stops its click's propagation still folds it.
    const foldUnlessTheme = (e: MouseEvent) => {
      if (document.documentElement.hasAttribute('data-overlay-open')) return;
      if (e.target instanceof Element && e.target.closest('.theme-card[role="radio"]')) return;
      toggleOpen(false);
    };
    document.addEventListener('click', foldUnlessTheme, true);
    return () => document.removeEventListener('click', foldUnlessTheme, true);
  }, [open]);
  const gallery = themeGallery.value;
  const showSkeleton = useDelayedLoading(gallery);
  if (gallery.status === 'failed') return <LoadableError noun="themes" error={gallery.error} />;

  const mode = paintedThemeMode.value;
  const active = currentThemeId();
  const themes = gallery.status === 'loaded' ? gallery.data.themes : null;
  // A preference naming a theme that is gone paints the default
  // (refreshActiveTheme), so the picker shows and checks the default too.
  const painted = themes?.find(theme => theme.id === active) ?? themes?.find(theme => theme.id === DEFAULT_THEME_ID);
  return (
    <div ref={root}>
      <LoadingFade
        showSkeleton={showSkeleton}
        skeleton={<ListSkeletonOf containerClass="theme-grid" count={1} row={() => <ThemeCard mode={mode} />} />}
      >
        {gallery.status === 'loaded' && (
          <>
            <Disclosure open={!open}>
              {painted && (
                <div class="theme-grid">
                  <ThemeCard
                    theme={painted}
                    defaults={gallery.data.defaults}
                    mode={mode}
                    themeCount={gallery.data.themes.length}
                    onClick={() => toggleOpen(true)}
                  />
                </div>
              )}
            </Disclosure>
            <Disclosure open={open}>
              <div class="theme-families" role="radiogroup" aria-label="Theme">
                {groupThemesByFamily(gallery.data.themes).map(group => (
                  <div key={group.name} class="theme-family" role="group" aria-label={group.name}>
                    <span class="theme-family-name" aria-hidden="true">{group.name}</span>
                    <div class="theme-grid">
                      {group.themes.map(theme => {
                        const selected = theme.id === painted?.id;
                        // An active theme that has this mode needs no pick, so its tap folds
                        // the grid. One without it still offers pickTheme's mode switch.
                        const folds = selected && theme.modes.includes(mode);
                        return (
                          <ThemeCard
                            key={theme.id}
                            theme={theme}
                            defaults={gallery.data.defaults}
                            mode={mode}
                            selected={selected}
                            onClick={folds ? () => toggleOpen(false) : () => void pickTheme(theme)}
                          />
                        );
                      })}
                    </div>
                  </div>
                ))}
              </div>
            </Disclosure>
          </>
        )}
      </LoadingFade>
    </div>
  );
}

/** A radio in the unfolded grid, or, given `themeCount`, the folded summary
 *  that unfolds it. */
function ThemeCard({ theme, defaults, mode, selected = false, themeCount, onClick }: {
  theme?: Theme;
  defaults?: ThemeModeMaps;
  mode: ResolvedThemeMode;
  selected?: boolean;
  themeCount?: number;
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
  const summary = themeCount !== undefined;

  return (
    <button
      type="button"
      role={summary ? undefined : 'radio'}
      aria-checked={summary ? undefined : selected}
      aria-expanded={summary ? false : undefined}
      class={`theme-card${selected ? ' selected' : ''}${summary ? ' theme-toggle' : ''}`}
      data-tooltip={tooltip || undefined}
      disabled={sk}
      onClick={onClick}
    >
      <SkBlock w="100%" h="5rem" round>
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
        {summary
          ? <span class="theme-toggle-count">{themeCount} themes<ChevronRightIcon size="1em" /></span>
          : label && <span class="theme-card-modes">{label}</span>}
      </span>
    </button>
  );
}
