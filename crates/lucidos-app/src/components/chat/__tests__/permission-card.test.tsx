import { describe, it, expect } from 'vitest';
import {
  BROAD_ALLOW_INEFFECTIVE,
  DEFAULT_PERMISSION_CHOICE,
  inputTouchesProtectedPath,
  narrowPattern,
  permissionButtonState,
  renderQuestion,
  resolvedChoice,
  sessionLabel,
} from '../PermissionCard';
import { vnodeToText } from './vnodeToText';

describe('DEFAULT_PERMISSION_CHOICE', () => {
  it('seeds keyboard focus on "Allow once", not the leading Deny button', () => {
    // This is what a reflex Enter does to a permission request, so it is pinned
    // deliberately rather than left to button order. `Deny` leads the row, but a
    // keyboard user answering a permission card overwhelmingly means to allow
    // the one thing being asked about. The grant is never hidden: the seeded
    // choice always carries a visible focus ring, per the choice-card contract
    // in choiceCardNav.ts. Changing this value is a security-relevant decision.
    expect(DEFAULT_PERMISSION_CHOICE).toBe('allow');
  });
});

describe('BROAD_ALLOW_INEFFECTIVE', () => {
  it('contains the tools whose bare allowlist entry is silently ignored by CC', () => {
    // Mirror of the engine-side BROAD_ALLOW_INEFFECTIVE constant in
    // claude_code.rs. If this test fails, update both sides together.
    // Edit/Write/NotebookEdit: acceptEdits mode routes them through the
    // permission prompt for protected paths regardless of --allowedTools.
    // ExitPlanMode: CC always routes plan-mode exit through the permission
    // prompt so the user can review the plan before approving.
    expect([...BROAD_ALLOW_INEFFECTIVE].sort()).toEqual([
      'Edit',
      'ExitPlanMode',
      'NotebookEdit',
      'Write',
    ]);
  });
});

describe('renderQuestion', () => {
  it('frames the tool name as <strong> and the path as <code>', () => {
    const text = vnodeToText(renderQuestion('Edit', 'Edit /Users/me/.claude/skills/x.md'));
    expect(text).toContain('the <strong>Edit</strong> tool');
    expect(text).toContain('on <code>/Users/me/.claude/skills/x.md</code>');
    expect(text).toContain('Allow?');
  });

  it('attributes the request to "the coding agent", not a specific backend', () => {
    // The same card is raised by CC's MCP permission prompt AND the Codex
    // app-server approval bridge — naming Claude Code would misattribute a
    // Codex escalation at the moment of a security decision.
    const text = vnodeToText(
      renderQuestion('command_execution', 'command_execution sudo ls', { command: 'sudo ls' }),
    );
    expect(text).toContain('The coding agent wants');
    expect(text).not.toContain('Claude Code');
  });

  it('omits the "on <arg>" clause when the summary has no argument', () => {
    const text = vnodeToText(renderQuestion('ExitPlanMode', 'ExitPlanMode'));
    expect(text).toContain('the <strong>ExitPlanMode</strong> tool');
    expect(text).not.toContain('<code>');
    expect(text).toContain('Allow?');
  });

  it('strips the leading category for Skill summaries (e.g. "skill meta:trace" → "meta:trace")', () => {
    const text = vnodeToText(renderQuestion('Skill', 'skill meta:trace'));
    expect(text).toContain('the <strong>Skill</strong> tool');
    expect(text).toContain('on <code>meta:trace</code>');
  });
});

describe('renderQuestion, the Codex backend tools', () => {
  const change = (path: string, type?: string) => ({
    path,
    ...(type ? { kind: { type } } : {}),
  });

  it('never puts a wire tool identifier in the sentence', () => {
    // The reported bug: a card reading "wants to use the file_change tool.
    // Allow?" and nothing else. `file_change` / `command_execution` are
    // app-server protocol names that surface nowhere else in the product, so
    // they must never be the words a user makes a security decision on.
    const cards = [
      renderQuestion('file_change', 'file_change /a.txt', { changes: [change('/a.txt', 'add')] }),
      renderQuestion('file_change', 'file_change', { item_id: 'exec-1' }),
      renderQuestion('command_execution', 'command_execution sudo ls', { command: 'sudo ls' }),
      renderQuestion('command_execution', 'command_execution', {}),
    ];
    for (const card of cards) {
      const text = vnodeToText(card);
      expect(text).not.toContain('file_change');
      expect(text).not.toContain('command_execution');
      expect(text).toContain('The coding agent wants to');
      expect(text).toContain('Allow?');
    }
  });

  it('names the single changed file, with a verb matching the change kind', () => {
    const verbs: [string | undefined, string][] = [
      ['add', 'create'],
      ['update', 'change'],
      ['delete', 'delete'],
      // An unrecognized or absent kind reads "change": true of every kind, and
      // it claims nothing the frame did not say.
      ['rename', 'change'],
      [undefined, 'change'],
    ];
    for (const [kind, verb] of verbs) {
      const text = vnodeToText(
        renderQuestion('file_change', 'file_change /Users/me/notes.txt', {
          changes: [change('/Users/me/notes.txt', kind)],
        }),
      );
      expect(text).toContain(`wants to ${verb} <code>/Users/me/notes.txt</code>`);
    }
  });

  it('counts a multi-file patch and lists every file under it', () => {
    const text = vnodeToText(
      renderQuestion('file_change', 'file_change /a.rs, /b.rs', {
        changes: [change('/a.rs', 'add'), change('/b.rs', 'add')],
      }),
    );
    expect(text).toContain('wants to create 2 files');
    expect(text).toContain('<code>/a.rs</code>');
    expect(text).toContain('<code>/b.rs</code>');
  });

  it('falls back to "change" for the whole set when the kinds disagree', () => {
    const text = vnodeToText(
      renderQuestion('file_change', 'file_change /a.rs, /b.rs', {
        changes: [change('/a.rs', 'add'), change('/b.rs', 'delete')],
      }),
    );
    expect(text).toContain('wants to change 2 files');
    // Each file still carries its own verb in the list.
    expect(text).toContain('create');
    expect(text).toContain('delete');
  });

  it('degrades to the least it can still claim when no paths were announced', () => {
    // Codex reordering or dropping the `item/started` that carries the paths
    // must cost the card its detail, never its correctness.
    const bare = vnodeToText(renderQuestion('file_change', 'file_change', { item_id: 'exec-1' }));
    expect(bare).toContain('The coding agent wants to change files. Allow?');
    expect(bare).not.toContain('<code>');

    const withReason = vnodeToText(
      renderQuestion('file_change', 'file_change needs write access', {
        reason: 'needs write access',
      }),
    );
    expect(withReason).toContain('wants to change files: needs write access. Allow?');
  });

  it('discards the whole set when one change entry has no readable path', () => {
    // Mirrors the engine's `FileTargets::Unresolved`. The driver writes an
    // omitted path as "", so a half-understood patch really arrives this way;
    // naming only the half that parsed would show a complete-looking card for
    // a patch whose other half writes somewhere nobody looked.
    const text = vnodeToText(
      renderQuestion('file_change', 'file_change', {
        changes: [change('/a.rs', 'add'), { path: '', kind: { type: 'add' } }],
      }),
    );
    expect(text).toContain('The coding agent wants to change files. Allow?');
    expect(text).not.toContain('/a.rs');
  });

  it('does not resolve a change kind off Object.prototype', () => {
    // The kind is a string codex chose, so it indexes the verb lookup with
    // whatever it sends. On a plain object literal `CHANGE_VERBS[k] ?? default`
    // answers `constructor` / `toString` / `valueOf` / `__proto__` off the
    // prototype and returns a FUNCTION, which would be rendered into the card.
    for (const kind of ['constructor', 'toString', 'valueOf', '__proto__', 'hasOwnProperty']) {
      const text = vnodeToText(
        renderQuestion('file_change', 'file_change /a.rs', {
          changes: [change('/a.rs', kind)],
        }),
      );
      expect(text).toContain('wants to change <code>/a.rs</code>');
      expect(text).not.toContain('native code');
      expect(text).not.toContain('function');
    }
  });

  it('asks about a command the same way the command-guard card does', () => {
    const text = vnodeToText(
      renderQuestion('command_execution', 'command_execution sudo rm -rf /x', {
        command: 'sudo rm -rf /x',
        cwd: '/wt',
      }),
    );
    expect(text).toContain('wants to run <code>sudo rm -rf /x</code>. Allow?');
  });
});

describe('sessionLabel', () => {
  it('returns the basename for Edit/Write so the button stays compact on long paths', () => {
    expect(
      sessionLabel('Edit', { file_path: '/Users/me/repo/.claude/commands/harden.md' }),
    ).toBe('harden.md');
    expect(sessionLabel('Write', { file_path: '/tmp/new.txt' })).toBe('new.txt');
  });

  it('uses notebook_path for NotebookEdit', () => {
    expect(sessionLabel('NotebookEdit', { notebook_path: '/tmp/nb.ipynb' })).toBe('nb.ipynb');
  });

  it('falls back to the full path when no slash is present', () => {
    expect(sessionLabel('Edit', { file_path: 'README.md' })).toBe('README.md');
  });

  it('returns "<first-token> …" for Bash so the user knows the scope is per-program, not per-command', () => {
    expect(sessionLabel('Bash', { command: 'git status --short' })).toBe('git …');
  });

  it('treats Codex command_execution like Bash (same per-program session scope)', () => {
    expect(sessionLabel('command_execution', { command: 'git push origin main' })).toBe('git …');
  });

  it('returns the plugin slug for Skill', () => {
    expect(sessionLabel('Skill', { skill: 'superpowers:test-driven-development' })).toBe(
      'superpowers:*',
    );
    expect(sessionLabel('Skill', { skill: 'loop' })).toBe('loop:*');
  });

  it('returns null when the input has no useful identifier', () => {
    expect(sessionLabel('Edit', {})).toBeNull();
    expect(sessionLabel('Bash', {})).toBeNull();
    expect(sessionLabel('Read', { file_path: '/x' })).toBeNull();
  });
});

describe('inputTouchesProtectedPath', () => {
  // Mirror of the engine-side `input_touches_protected_path` helper.
  // Restricted to Bash because that's the only tool empirically observed
  // to surface a card on these paths under bare allowlist entries (see
  // `CC_PROTECTED_PATH_MARKERS` doc in claude_code.rs for the probing
  // summary). Read / Edit / cat on `~/.claude/skills/...` all
  // auto-approved silently — so the filter would be dead code for them.
  it('detects .claude/ inside Bash command', () => {
    expect(
      inputTouchesProtectedPath('Bash', { command: 'rm -rf /Users/me/.claude/skills/grill' }),
    ).toBe(true);
  });

  it('detects .git/ inside Bash command', () => {
    expect(inputTouchesProtectedPath('Bash', { command: 'cat .git/HEAD' })).toBe(true);
  });

  it('returns false for unrelated Bash commands', () => {
    expect(inputTouchesProtectedPath('Bash', { command: 'git status --short' })).toBe(false);
  });

  it('does not match .gitignore or .claude_backup (trailing slash anchor)', () => {
    expect(inputTouchesProtectedPath('Bash', { command: 'cat .gitignore' })).toBe(false);
    expect(inputTouchesProtectedPath('Bash', { command: 'ls .claude_backup' })).toBe(false);
  });

  it('returns false for non-Bash tools regardless of input', () => {
    // Read/Edit/Glob/Grep/NotebookEdit on `~/.claude/skills/...`
    // auto-approved silently under bare allowlist entries — so the
    // helper is Bash-only by design.
    const path = '/Users/me/repo/.claude/commands/harden.md';
    expect(inputTouchesProtectedPath('Edit', { file_path: path })).toBe(false);
    expect(inputTouchesProtectedPath('Write', { file_path: path })).toBe(false);
    expect(inputTouchesProtectedPath('Read', { file_path: path })).toBe(false);
    expect(
      inputTouchesProtectedPath('NotebookEdit', { notebook_path: '/repo/.git/x.ipynb' }),
    ).toBe(false);
    expect(inputTouchesProtectedPath('Glob', { pattern: '.claude/skills/**/*.md' })).toBe(false);
    expect(inputTouchesProtectedPath('Grep', { path: '/repo/.git/', pattern: 'HEAD' })).toBe(false);
    expect(inputTouchesProtectedPath('Skill', { skill: 'code-review:code-review' })).toBe(false);
  });
});

describe('narrowPattern', () => {
  it('returns null when Bash command targets a CC-protected path', () => {
    expect(
      narrowPattern('Bash', { command: 'rm -rf /Users/me/.claude/skills/grill' }),
    ).toBeNull();
  });

  it('returns Bash(<token>:*) for ordinary commands', () => {
    expect(narrowPattern('Bash', { command: 'git status' })).toBe('Bash(git:*)');
  });

  it('returns Skill(<plugin>:*) for Skill', () => {
    expect(narrowPattern('Skill', { skill: 'superpowers:test' })).toBe('Skill(superpowers:*)');
  });

  it('returns null for tools without a narrow scope', () => {
    expect(narrowPattern('Edit', { file_path: '/x' })).toBeNull();
    expect(narrowPattern('Read', { file_path: '/x' })).toBeNull();
  });
});

describe('permissionButtonState', () => {
  it('returns enabled with no stateClass when not answered and not terminated', () => {
    expect(permissionButtonState({ answered: false, terminated: false, isPicked: false }))
      .toEqual({ disabled: false, stateClass: '' });
  });

  it('marks the picked button with permission-btn-picked when answered', () => {
    expect(permissionButtonState({ answered: true, terminated: false, isPicked: true }))
      .toEqual({ disabled: true, stateClass: ' permission-btn-picked' });
  });

  it('marks the non-picked buttons with permission-btn-rejected when answered', () => {
    expect(permissionButtonState({ answered: true, terminated: false, isPicked: false }))
      .toEqual({ disabled: true, stateClass: ' permission-btn-rejected' });
  });

  it('disables every button without picked/rejected styling when terminated but unanswered', () => {
    expect(permissionButtonState({ answered: false, terminated: true, isPicked: false }))
      .toEqual({ disabled: true, stateClass: '' });
    expect(permissionButtonState({ answered: false, terminated: true, isPicked: true }))
      .toEqual({ disabled: true, stateClass: '' });
  });

  // answered+terminated: keep the recorded decision visible even after a
  // later abort lands.
  it('keeps picked/rejected styling when answered even if terminated also true', () => {
    expect(permissionButtonState({ answered: true, terminated: true, isPicked: true }))
      .toEqual({ disabled: true, stateClass: ' permission-btn-picked' });
    expect(permissionButtonState({ answered: true, terminated: true, isPicked: false }))
      .toEqual({ disabled: true, stateClass: ' permission-btn-rejected' });
  });
});

describe('resolvedChoice', () => {
  it('maps a deny to "deny" regardless of scope', () => {
    expect(resolvedChoice({ allowed: false })).toBe('deny');
    // Recovery-emitted orphans arrive as `allowed: false` with a reason but
    // no scope — still "deny" so the answered card marks the Deny button.
    expect(resolvedChoice({ allowed: false, reason: 'orphan' } as { allowed: boolean })).toBe('deny');
  });

  it('maps allow + no scope to "allow" (the bare Allow-once button)', () => {
    expect(resolvedChoice({ allowed: true })).toBe('allow');
  });

  it('maps allow + scope to the scope name', () => {
    expect(resolvedChoice({ allowed: true, persist_scope: 'session' })).toBe('session');
    expect(resolvedChoice({ allowed: true, persist_scope: 'narrow' })).toBe('narrow');
    expect(resolvedChoice({ allowed: true, persist_scope: 'broad' })).toBe('broad');
  });
});
