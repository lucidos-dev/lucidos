/**
 * Every frame the engine can broadcast is either answered by the frontend or
 * written down as answering nothing. This is the receiving mirror of
 * `core/announced_surfaces.rs`, which holds the emitting half (ADR 0032), and
 * the enforcement behind ADR 0118.
 *
 * The reported bug was a trigger detail page that ignored a `TriggerUpdated`
 * frame its own store had already applied. The event WAS handled, so this
 * guard would not have caught that one. It catches the other half of the same
 * class: the event nobody wired at all, which is how MCP servers, webhooks and
 * the permission allowlists went a whole session stale.
 *
 * `RESERVED_TYPE_NAMES` is read out of the Rust source rather than restated
 * here, and `reserved_type_names_match_event_type` guards that const against
 * the enum. So a new `SystemEvent` variant reaches this test with no step in
 * between.
 */
import { describe, it, expect } from 'vitest';
// @ts-expect-error: Node APIs available at runtime via Vitest, no @types/node in project
import { readFileSync } from 'node:fs';
// @ts-expect-error: same
import { fileURLToPath } from 'node:url';
// @ts-expect-error: same
import { dirname, resolve } from 'node:path';

const here = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(here, '../../../../..');

const SYSTEM_EVENT_RS = 'crates/lucidos-engine/src/engine/event_bus_system_event.rs';

/** The two SSE dispatchers. Every `case '…':` in them is a handled frame. */
const DISPATCHERS = ['thread-sync.ts', 'entityReferences.ts'];

/** A frame that drives no UI state, and why. Each entry is a decision, so the
 *  next reader re-decides rather than re-discovers. Adding a row is the way to
 *  make this test pass WITHOUT an arm, and it should be harder to justify than
 *  writing the arm. */
const NO_UI_STATE: Record<string, string> = {
  DomainEvent:
    'a transport wrapper for a workspace-defined event, stored under the inner '
    + 'name. The shell holds no state keyed on one; apps subscribe through the SDK',
  TriggerCompleted:
    'the narrative summary of a threadless run. The row that shows an outcome '
    + 'moves on `TriggerExecuted`, which carries the ok/failed status',
  ChangeDiscarded:
    'a name shared with a ThreadEvent variant; nothing emits the SystemEvent '
    + 'form. The changes list moves on `ChangesUpdated`',
  DeviceVisible:
    'this device reporting its own visibility. The engine consumes it for '
    + 'cross-device push suppression and no surface reads it back',
  DeviceHidden: 'the other half of `DeviceVisible`',
  PluginLocalChangesMerged:
    'the audit record of what an update did to local edits. It always rides '
    + 'with `PluginInstalled`, which is what refreshes the lists',
  CredentialRevealed: 'an audit record of a read. Nothing changed',
  EmailSent: 'an audit record of a send. No surface lists sent mail',
  EngineSupervisorRespawned:
    'emitted at boot, for the timeline. Every client reconnecting after that '
    + 'boot re-reads its state anyway',
  ProxyModulesReloaded:
    'the proxy signer modules were reloaded in the engine. They have no page',
};

function read(path: string): string {
  return readFileSync(resolve(REPO_ROOT, path), 'utf8');
}

/** The wire names the engine reserves, straight out of the Rust const. */
function reservedTypeNames(): string[] {
  const src = read(SYSTEM_EVENT_RS);
  const block = /pub const RESERVED_TYPE_NAMES: &'static \[&'static str\] = &\[([\s\S]*?)\n {4}\];/
    .exec(src);
  if (!block) throw new Error(`RESERVED_TYPE_NAMES not found in ${SYSTEM_EVENT_RS}`);
  return [...block[1].matchAll(/"([A-Za-z0-9_]+)"/g)].map((m) => m[1]);
}

/** Every `case '…':` label in the two dispatchers. */
function handledTypeNames(): Set<string> {
  const handled = new Set<string>();
  for (const file of DISPATCHERS) {
    const src = readFileSync(resolve(here, file), 'utf8');
    for (const m of src.matchAll(/case '([A-Za-z0-9_]+)':/g)) handled.add(m[1]);
  }
  return handled;
}

describe('every SystemEvent is answered or written down', () => {
  const reserved = reservedTypeNames();
  const handled = handledTypeNames();

  it('reads a plausible list of wire names out of the Rust source', () => {
    // A regex that silently matched nothing would make every assertion below
    // pass while checking nothing at all.
    expect(reserved.length).toBeGreaterThan(50);
    expect(reserved).toContain('TriggerUpdated');
    expect(handled.size).toBeGreaterThan(30);
  });

  it('leaves no frame unanswered and unexplained', () => {
    const orphans = reserved.filter((name) => !handled.has(name) && !(name in NO_UI_STATE));
    expect(
      orphans,
      'These frames reach the browser and nothing reads them. Give each one an '
      + `arm in ${DISPATCHERS.join(' or ')}, or a row in NO_UI_STATE saying what `
      + `state it does not touch: ${orphans.join(', ')}`,
    ).toEqual([]);
  });

  it('keeps no excuse for a frame that is handled after all', () => {
    const stale = Object.keys(NO_UI_STATE).filter((name) => handled.has(name));
    expect(
      stale,
      'These have an arm AND a NO_UI_STATE row. Drop the row: a reason nobody '
      + `relies on is a reason nobody rechecks: ${stale.join(', ')}`,
    ).toEqual([]);
  });

  it('keeps no excuse for a frame the engine can no longer send', () => {
    const gone = Object.keys(NO_UI_STATE).filter((name) => !reserved.includes(name));
    expect(
      gone,
      `These are in NO_UI_STATE but no longer reserved by the engine: ${gone.join(', ')}`,
    ).toEqual([]);
  });

  it('names every surface this change wired, so removing one is loud', () => {
    for (const name of [
      'McpServerRegistered', 'McpServerUpdated', 'McpServerRemoved',
      'McpServerDisabledToolsChanged',
      'WebhookCreated', 'WebhookUpdated', 'WebhookDeleted',
      'PermissionGrantsChanged',
      'PluginInstallCanceled', 'PluginUninstallCanceled',
    ]) {
      expect(handled.has(name), `${name} lost its arm`).toBe(true);
    }
  });
});

describe('no surface answers a frame by polling instead', () => {
  // The one path. A timer or a focus listener makes the symptom go away and
  // leaves the rule unenforced, so the next surface repeats the bug.
  // `device-presence.ts` legitimately owns the visibility listeners: it
  // REPORTS this device's presence rather than reading state.
  const BANNED = [/setInterval\s*\(/, /'visibilitychange'/, /'focus'\s*,/];
  const SUBSCRIBERS = [
    'entityReferences.ts',
    '../../hooks/useVersionedRefresh.ts',
    '../../hooks/useServerBackedField.ts',
  ];

  it.each(SUBSCRIBERS)('%s refreshes from a frame, never from a clock', (file) => {
    const src = readFileSync(resolve(here, file), 'utf8');
    for (const pattern of BANNED) {
      expect(pattern.test(src), `${file} matches ${pattern}`).toBe(false);
    }
  });
});
