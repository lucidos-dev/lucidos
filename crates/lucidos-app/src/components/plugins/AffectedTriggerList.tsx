import type { ComponentChildren } from 'preact';
import type { AffectedPath, AffectedTrigger } from '../../store/types';
import { PluginSection } from './PluginSection';

/** Why a trigger relies on a removed path, in the user's words. An owning-app
 *  match names the app by its folder, since the app may be gone already. */
export function affectedReasonLabel({ path, reason }: AffectedPath): string {
  switch (reason) {
    case 'named-in-run':
      return 'Named in what it runs';
    case 'owning-app':
      return `Belongs to the ${path.split('/')[1]} app`;
    case 'trigger-knowhow':
      return 'Its own know-how';
  }
}

/** The *affected triggers* of a plugin update or uninstall: each trigger, the
 *  removed paths it relies on, and why. Open by default, because the engine
 *  leaves these triggers alone and this list is the only warning before the
 *  next fire. Every plugin panel and receipt lists them through here. */
export function AffectedTriggerList({
  label,
  triggers,
  tone,
  note,
}: {
  label: string;
  triggers: AffectedTrigger[];
  tone?: 'danger';
  note: ComponentChildren;
}) {
  return (
    <PluginSection label={label} count={triggers.length} tone={tone} note={note} defaultOpen>
      <ul class="plugin-install-files">
        {triggers.map((t) => (
          <li class="plugin-affected-trigger" key={t.trigger_id}>
            <span class="plugin-affected-trigger-name">{t.trigger_name}</span>
            {t.paths.map((p) => (
              <span class="plugin-affected-trigger-path" key={p.path}>
                <span class="plugin-affected-trigger-file">{p.path}</span>
                <span class="plugin-install-outcome">{affectedReasonLabel(p)}</span>
              </span>
            ))}
          </li>
        ))}
      </ul>
    </PluginSection>
  );
}

/** What happens next, said once for every surface that lists them. */
export const AFFECTED_TRIGGERS_NEXT_RUN =
  'Their next run warns that this know-how is gone until you edit them.';
