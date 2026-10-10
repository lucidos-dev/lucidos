import { useState } from 'preact/hooks';
import { appFileUrl } from '../../api/client/apps';
import { appsList } from '../../store/store';

/** The theme hues a monogram tile may wear. Each is a theme variable, so the
 *  tile follows the theme (ADR 0414). */
export const MONOGRAM_HUES = [
  '--accent',
  '--accent-green',
  '--accent-yellow',
  '--accent-red',
  '--accent-orange',
  '--accent-notable',
] as const;

/** FNV-1a over the id's code points, so an app keeps its hue on every device. */
export function monogramHue(appId: string): (typeof MONOGRAM_HUES)[number] {
  let hash = 2166136261;
  for (const char of appId) {
    hash ^= char.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 16777619);
  }
  return MONOGRAM_HUES[(hash >>> 0) % MONOGRAM_HUES.length];
}

/** The tile's letter: the name's first character, whole even when it is not
 *  one UTF-16 unit. */
export function monogramLetter(name: string): string {
  const [first] = name.trim();
  return first ? first.toLocaleUpperCase() : '?';
}

interface AppIconProps {
  /** The id the monogram hue keys on: an app's, or a plugin's. */
  appId: string;
  name: string;
  /** The validated `App.icon`: a path inside the app folder. */
  icon?: string;
  /** A ready URL that wins over `icon`, for a picture that is not an app
   *  file: a plugin icon on the media route. */
  src?: string;
  class?: string;
}

/** The app icon of an app or widget, or a plugin icon, on a tile a surface
 *  sizes through `--app-icon-size`. A missing icon, or one that fails to load, shows the
 *  monogram tile. Every surface draws an app picture through this component.
 *  A failure holds until the apps list is read again. An app change re-reads
 *  it, and may have repaired the file at the same path. */
export function AppIcon({ appId, name, icon, src: given, class: extra }: AppIconProps) {
  const listing = appsList.value;
  const [failed, setFailed] = useState<{ src: string; listing: typeof listing } | null>(null);
  const src = given ?? (icon ? appFileUrl(appId, icon) : null);
  const cls = extra ? `app-icon ${extra}` : 'app-icon';
  if (src && !(failed?.src === src && failed.listing === listing)) {
    return (
      <span class={`${cls} app-icon-image`} aria-hidden="true">
        <img src={src} alt="" draggable={false} onError={() => setFailed({ src, listing })} />
      </span>
    );
  }
  return (
    <span
      class={`${cls} app-icon-monogram`}
      style={{ '--app-icon-hue': `var(${monogramHue(appId)})` }}
      aria-hidden="true"
    >
      {monogramLetter(name)}
    </span>
  );
}

/** `AppIcon` for a surface that knows only the app's id, such as a search hit.
 *  It reads the icon from the apps list, and shows the monogram until that
 *  list has loaded. */
export function ListedAppIcon({ appId, name, class: extra }: Omit<AppIconProps, 'icon'>) {
  const apps = appsList.value;
  const icon = apps.status === 'loaded' ? apps.data.find((a) => a.id === appId)?.icon : undefined;
  return <AppIcon appId={appId} name={name} icon={icon} class={extra} />;
}
