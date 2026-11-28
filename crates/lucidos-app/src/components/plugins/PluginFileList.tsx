import type { ComponentChildren } from 'preact';
import { PluginSection } from './PluginSection';

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
