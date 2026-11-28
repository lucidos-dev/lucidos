import { useState } from 'preact/hooks';
import { appFileUrl } from '../../api/client/apps';
import { appsList } from '../../store/store';
import type { App } from '../../store/types';

export const MONOGRAM_HUES = [
  '--accent',
  '--accent-green',
  '--accent-yellow',
  '--accent-red',
  '--accent-orange',
  '--accent-notable',
] as const;

export function monogramHue(appId: string): (typeof MONOGRAM_HUES)[number] {
  let hash = 2166136261;
  for (const char of appId) {
    hash ^= char.codePointAt(0) ?? 0;
    hash = Math.imul(hash, 16777619);
  }
  return MONOGRAM_HUES[(hash >>> 0) % MONOGRAM_HUES.length];
}

export function monogramLetter(name: string): string {
  const [first] = name.trim();
  return first ? first.toLocaleUpperCase() : '?';
}

interface AppIconProps {
  appId: string;
  name: string;
  icon?: string;
  src?: string;
  class?: string;
}

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

const warmedIcons = new Set<string>();

export function warmAppIcons(apps: App[]): void {
  for (const app of apps) {
    if (!app.icon) continue;
    const src = appFileUrl(app.id, app.icon);
    if (warmedIcons.has(src)) continue;
    warmedIcons.add(src);
    new Image().src = src;
  }
}

export function ListedAppIcon({ appId, name, class: extra }: Omit<AppIconProps, 'icon'>) {
  const apps = appsList.value;
  const icon = apps.status === 'loaded' ? apps.data.find((a) => a.id === appId)?.icon : undefined;
  return <AppIcon appId={appId} name={name} icon={icon} class={extra} />;
}
