/** The `data/` sub-trees the `/data/*` mount serves: the engine's
 *  `MUTABLE_PREFIXES` in `core/data_prefixes.rs`, plus the read-only
 *  `system-knowhow/`. `every_data_prefix_list_matches_the_engine` in the CLI
 *  crate pins it to the engine's list.
 *
 *  It drives the href recognizer in `linkifyPaths.ts`, relative image sources, and
 *  `normalizeDataPath`, which prefixes anything else with `artifacts/`. A tree
 *  missing here opens the wrong file, and a link to it reloads the workspace.
 *
 *  Its own module because the data layer reads it too, and the linkifier
 *  would otherwise ride along into the entry chunk (ADR 0288). */
export const DATA_PATH_PREFIXES: readonly string[] = [
  'artifacts/',
  'apps/',
  'knowhow/',
  'triggers/',
  'config/',
  'auth-modules/',
  'scripts/',
  'themes/',
  'fonts/',
  'system-knowhow/',
];
