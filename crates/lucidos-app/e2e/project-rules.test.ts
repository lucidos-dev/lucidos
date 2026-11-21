import { execFileSync } from 'node:child_process';
import { readdirSync, readFileSync } from 'node:fs';
import { join, resolve } from 'node:path';
import { describe, expect, it } from 'vitest';
import { DESKTOP_IGNORES, PHONE_SPECS } from './projectRules';

const E2E_DIR = __dirname;
const E2E_LIB = resolve(E2E_DIR, '../../../scripts/lib/e2e_projects.sh');
const PROJECTS = ['chromium', 'mobile', 'mobile-webkit'] as const;
type Project = (typeof PROJECTS)[number];

const SPECS = readdirSync(E2E_DIR).filter(file => file.endsWith('.spec.ts')).sort();

/** What playwright.config.ts decides, from the same patterns it applies. */
function configRuns(project: Project, spec: string): boolean {
  return project === 'chromium' ? !DESKTOP_IGNORES.test(spec) : PHONE_SPECS.test(spec);
}

/** What the shell mirror decides, one `<project> <spec> yes|no` line per pair.
 *  Lifts the one function rather than sourcing the whole library. */
function shellRuns(): Set<string> {
  const script = `
    eval "$(sed -n '/^project_runs_spec() {/,/^}/p' "$1")"
    shift
    for project in ${PROJECTS.join(' ')}; do
      for spec in "$@"; do
        if project_runs_spec "$project" "$spec"; then echo "$project $spec"; fi
      done
    done`;
  const out = execFileSync('bash', ['-c', script, 'bash', E2E_LIB, ...SPECS], { encoding: 'utf8' });
  return new Set(out.split('\n').filter(Boolean));
}

/** What a spec looks like when it means to behave differently on a phone or
 *  in WebKit. A spec that only desktop Chromium runs never takes that branch.
 *  The navigation shims (`ensureMobileView`, `waitForPaneAtRest`) are no-ops on
 *  desktop and stay allowed. */
const PLATFORM_BRANCHES: { name: string; pattern: RegExp }[] = [
  { name: 'isMobile', pattern: /\bisMobile\b/ },
  { name: 'isMobileViewport', pattern: /\bisMobileViewport\b/ },
  { name: 'browserName', pattern: /\bbrowserName\b/ },
  { name: 'project.name', pattern: /\bproject\.name\b/ },
  { name: 'hasTouch', pattern: /\bhasTouch\b/ },
  { name: 'tap()', pattern: /\.tap\(/ },
  { name: 'touchscreen', pattern: /\btouchscreen\b/ },
];

/** Code only: a comment may name a phone or WebKit to explain the spec. */
function withoutComments(src: string): string {
  return src.replace(/\/\*[\s\S]*?\*\//g, '').replace(/\/\/.*$/gm, '');
}

function platformBranches(src: string): string[] {
  const code = withoutComments(src);
  return PLATFORM_BRANCHES.filter(b => b.pattern.test(code)).map(b => b.name);
}

describe('browser project rules', () => {
  it('the shell mirror agrees with playwright.config.ts for every spec', () => {
    const shell = shellRuns();
    const disagreements = PROJECTS.flatMap(project =>
      SPECS.filter(spec => configRuns(project, spec) !== shell.has(`${project} ${spec}`))
        .map(spec => `${project} ${spec}: config ${configRuns(project, spec)}, shell ${shell.has(`${project} ${spec}`)}`));
    expect(SPECS.length).toBeGreaterThan(0);
    expect(disagreements).toEqual([]);
  });

  it('every spec runs on at least one project', () => {
    const orphans = SPECS.filter(spec => !PROJECTS.some(project => configRuns(project, spec)));
    expect(orphans).toEqual([]);
  });

  it('a spec that only desktop Chromium runs carries no phone or WebKit branch', () => {
    const branching = SPECS
      .filter(spec => !PHONE_SPECS.test(spec))
      .flatMap(spec => {
        const found = platformBranches(readFileSync(join(E2E_DIR, spec), 'utf8'));
        return found.length > 0 ? [`${spec}: ${found.join(', ')}`] : [];
      });
    expect(branching, 'rename the spec -all or -mobile, or drop the dead branch').toEqual([]);
  });

  it('spots each kind of platform branch', () => {
    expect(platformBranches('test.skip(isMobileViewport(page))')).toEqual(['isMobileViewport']);
    expect(platformBranches("async ({ page, browserName }) => {}")).toEqual(['browserName']);
    expect(platformBranches("testInfo.project.name === 'mobile-webkit'")).toEqual(['project.name']);
    expect(platformBranches('await button.tap();')).toEqual(['tap()']);
    expect(platformBranches("await ensureMobileView(page, 'thread');")).toEqual([]);
    expect(platformBranches('// the phone projects pin `isMobile: true`')).toEqual([]);
  });

  it('reads each suffix the way the decision says', () => {
    expect(PROJECTS.filter(p => configRuns(p, 'chat.spec.ts'))).toEqual(['chromium']);
    expect(PROJECTS.filter(p => configRuns(p, 'header-desktop.spec.ts'))).toEqual(['chromium']);
    expect(PROJECTS.filter(p => configRuns(p, 'swipe-mobile.spec.ts'))).toEqual(['mobile', 'mobile-webkit']);
    expect(PROJECTS.filter(p => configRuns(p, 'drafts-all.spec.ts'))).toEqual(PROJECTS);
  });
});
