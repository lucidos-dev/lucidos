import type { ComponentChildren } from 'preact';
import { PluginSection } from './PluginSection';

/** A folded list of `data/`-relative plugin paths, counted beside its label.
 *  Every plugin panel and receipt lists its files through here, so a panel and
 *  its receipt present the same paths the same way. */
export function PluginFileList({
  label,
  files,
  tone,
  note,
}: {
  label: string;
  files: string[];
  tone?: 'danger';
  note?: ComponentChildren;
}) {
  return (
    <PluginSection label={label} count={files.length} tone={tone} note={note}>
      <ul class="plugin-install-files">
        {files.map((f) => (
          <li class="plugin-install-file" key={f}>{f}</li>
        ))}
      </ul>
    </PluginSection>
  );
}
