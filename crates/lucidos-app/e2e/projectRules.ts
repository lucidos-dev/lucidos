/** Which browser projects run a spec, by its file suffix
 *  (docs/e2e-test-decisions.md § One browser by default):
 *
 *    no suffix, -desktop   desktop Chromium only
 *    -mobile               the two phone projects
 *    -all                  all three
 *
 *  playwright.config.ts applies these. scripts/lib/e2e_projects.sh `project_runs_spec`
 *  restates them in shell, and project-rules.test.ts holds the two together. */

/** The specs desktop Chromium skips. */
export const DESKTOP_IGNORES = /-mobile\.spec\.ts$/;

/** The specs the phone projects run. */
export const PHONE_SPECS = /-(mobile|all)\.spec\.ts$/;
