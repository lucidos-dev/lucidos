import type { ComponentChildren } from 'preact';

/** A labelled list of `data/`-relative plugin paths.
 *
 *  Six of these render across the install and uninstall panels and their
 *  receipts (overwrites, new files, will-be-deleted, already-gone, deleted,
 *  installed), all identical but for the heading, an optional emphasis class on
 *  the rows, and an optional note underneath. Keeping one component means a
 *  panel and its receipt cannot drift apart in how they present the same paths.
 *
 *  Callers pass the count in `label`: the two of them that show one word for a
 *  count of one read better spelled out at the call site than behind a
 *  pluralisation prop. */
export function PluginFileList({
  label,
  files,
  sectionClass,
  fileClass,
  note,
}: {
  label: string;
  files: string[];
  /** Extra class on the wrapping section, e.g. the overwrite emphasis. */
  sectionClass?: string;
  /** Extra class on each row, e.g. the overwrite emphasis. */
  fileClass?: string;
  note?: ComponentChildren;
}) {
  return (
    <section class={`plugin-install-section${sectionClass ? ` ${sectionClass}` : ''}`}>
      <div class="plugin-install-label">{label}</div>
      <ul class="plugin-install-files">
        {files.map((f) => (
          <li class={`plugin-install-file${fileClass ? ` ${fileClass}` : ''}`} key={f}>{f}</li>
        ))}
      </ul>
      {note && <p class="plugin-install-warning-text">{note}</p>}
    </section>
  );
}
