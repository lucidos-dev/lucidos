import { describe, it, expect } from 'vitest';
import {
  DEFAULT_PERMISSION_CHOICE,
  permissionButtonState,
  grantLabel,
  REASON_HEADING,
  renderCommandQuestion,
  renderMcpQuestion,
  renderQuestion,
  resolvedChoice,
  sessionLabel,
} from '../PermissionCard';
// @ts-expect-error: Node APIs exist at runtime under Vitest, and the project has no @types/node.
import { readFileSync } from 'node:fs';
import { vnodeToText } from './vnodeToText';

describe('DEFAULT_PERMISSION_CHOICE', () => {
  it('seeds keyboard focus on "Allow once", not the leading Deny button', () => {
    expect(DEFAULT_PERMISSION_CHOICE).toBe('allow');
  });
});

describe('renderQuestion', () => {
  it('says what a known tool does, and frames the path as <code>', () => {
    const text = vnodeToText(renderQuestion('Edit', 'Edit /Users/me/.claude/skills/x.md'));
    expect(text).toContain('Let the coding agent edit <code>/Users/me/.claude/skills/x.md</code>?');
    expect(text).not.toContain('<code>Edit</code>');
  });

  it('names an unknown tool as a tool, and a tool-server tool by tool and server', () => {
    expect(vnodeToText(renderQuestion('Frobnicate', 'Frobnicate x'))).toContain('use the <code>Frobnicate</code> tool on <code>x</code>?');
    const mcp = vnodeToText(renderQuestion('mcp__slack__post_message', 'mcp__slack__post_message'));
    expect(mcp).toContain('use <code>post_message</code> from slack?');
    expect(mcp).not.toContain('mcp__');
  });

  it('attributes the request to "the coding agent", not a specific backend', () => {
    const text = vnodeToText(
      renderQuestion('command_execution', 'command_execution sudo ls', { command: 'sudo ls' }),
    );
    expect(text).toContain('Let the coding agent');
    expect(text).not.toContain('Claude Code');
  });

  it('omits the argument when the summary has none', () => {
    const text = vnodeToText(renderQuestion('ExitPlanMode', 'ExitPlanMode'));
    expect(text).toContain('Let the coding agent stop planning and start work on its plan?');
    expect(text).not.toContain('<code>');
    expect(text).not.toContain('ExitPlanMode');
  });

  it('strips the leading category for Skill summaries (e.g. "skill meta:trace" → "meta:trace")', () => {
    const text = vnodeToText(renderQuestion('Skill', 'skill meta:trace'));
    expect(text).toContain('Let the coding agent use the skill <code>meta:trace</code>?');
  });
});

describe('renderQuestion, the Codex backend tools', () => {
  const change = (path: string, type?: string) => ({
    path,
    ...(type ? { kind: { type } } : {}),
  });

  it('never puts a wire tool identifier in the sentence', () => {
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
      expect(text).toContain('Let the coding agent');
    }
  });

  it('names the single changed file, with a verb matching the change kind', () => {
    const verbs: [string | undefined, string][] = [
      ['add', 'create'],
      ['update', 'change'],
      ['delete', 'delete'],
      ['rename', 'change'],
      [undefined, 'change'],
    ];
    for (const [kind, verb] of verbs) {
      const text = vnodeToText(
        renderQuestion('file_change', 'file_change /Users/me/notes.txt', {
          changes: [change('/Users/me/notes.txt', kind)],
        }),
      );
      expect(text).toContain(`Let the coding agent ${verb} <code>/Users/me/notes.txt</code>?`);
    }
  });

  it('counts a multi-file patch and lists every file under it', () => {
    const text = vnodeToText(
      renderQuestion('file_change', 'file_change /a.rs, /b.rs', {
        changes: [change('/a.rs', 'add'), change('/b.rs', 'add')],
      }),
    );
    expect(text).toContain('Let the coding agent create these 2 files?');
    expect(text).toContain('<ul><li>create <code>/a.rs</code></li><li>create <code>/b.rs</code></li></ul>');
  });

  it('falls back to "change" for the whole set when the kinds disagree', () => {
    const text = vnodeToText(
      renderQuestion('file_change', 'file_change /a.rs, /b.rs', {
        changes: [change('/a.rs', 'add'), change('/b.rs', 'delete')],
      }),
    );
    expect(text).toContain('Let the coding agent change these 2 files?');
    expect(text).toContain('create');
    expect(text).toContain('delete');
  });

  it('degrades to the least it can still claim when no paths were announced', () => {
    const bare = vnodeToText(renderQuestion('file_change', 'file_change', { item_id: 'exec-1' }));
    expect(bare).toContain('Let the coding agent change files?');
    expect(bare).not.toContain('<code>');
    expect(bare).not.toContain('<blockquote>');

    const withReason = vnodeToText(
      renderQuestion('file_change', 'file_change needs write access', {
        reason: 'needs write access',
      }),
    );
    expect(withReason).toContain('Let the coding agent change files?');
    expect(withReason).toContain(`<blockquote><p><strong>${REASON_HEADING}</strong></p><p>needs write access</p></blockquote>`);
  });

  it('discards the whole set when one change entry has no readable path', () => {
    const text = vnodeToText(
      renderQuestion('file_change', 'file_change', {
        changes: [change('/a.rs', 'add'), { path: '', kind: { type: 'add' } }],
      }),
    );
    expect(text).toContain('Let the coding agent change files?');
    expect(text).not.toContain('/a.rs');
  });

  it('does not resolve a change kind off Object.prototype', () => {
    for (const kind of ['constructor', 'toString', 'valueOf', '__proto__', 'hasOwnProperty']) {
      const text = vnodeToText(
        renderQuestion('file_change', 'file_change /a.rs', {
          changes: [change('/a.rs', kind)],
        }),
      );
      expect(text).toContain('Let the coding agent change <code>/a.rs</code>?');
      expect(text).not.toContain('native code');
      expect(text).not.toContain('function');
    }
  });

  it('puts codex\'s reason in the reason box, under the whole command', () => {
    const text = vnodeToText(
      renderQuestion('command_execution', 'command_execution sudo rm -rf /x', {
        command: 'sudo rm -rf /x',
        cwd: '/wt',
        reason: 'Remove the stale build dir, so the next build starts clean',
      }),
    );
    expect(text).toContain('<pre><code>sudo rm -rf /x</code></pre><blockquote>');
    expect(text).toContain('<p>Remove the stale build dir, so the next build starts clean</p></blockquote>');
  });
});

describe('renderQuestion, a Claude Code shell command', () => {
  const longCommand =
    "sed -n '/^## Triggers/,+3p' /Users/me/.claude/projects/x/tool-results/abc.txt | head -40; lucidos knowhow list 2>/dev/null | grep -i trigger";

  it('puts the agent\'s own description in the reason box, after the command', () => {
    const text = vnodeToText(
      renderQuestion('Bash', `Bash ${longCommand}`, {
        command: longCommand,
        description: 'Read the Triggers section of the saved output, to see how triggers get installed',
      }),
    );
    expect(text).toContain(`<blockquote><p><strong>${REASON_HEADING}</strong></p><p>Read the Triggers section of the saved output, to see how triggers get installed</p></blockquote>`);
    expect(text.indexOf('<pre>')).toBeLessThan(text.indexOf('Read the Triggers'));
  });

  it('shows the command verbatim under the description, never cut short', () => {
    const text = vnodeToText(
      renderQuestion('Bash', `Bash ${longCommand}`, {
        command: longCommand,
        description: 'Read the Triggers section',
      }),
    );
    expect(text).toContain(`<code>${longCommand}</code>`);
  });

  it('shows the command byte for byte, surrounding whitespace included', () => {
    const script = "\npython3 - <<'EOF'\nprint(1)\nEOF\n";
    const text = vnodeToText(
      renderQuestion('Bash', 'Bash python3', { command: script, description: 'Print 1' }),
    );
    expect(text).toContain(`<code>${script}</code>`);
  });

  it('keeps the command out of the sentence', () => {
    const text = vnodeToText(
      renderQuestion('Bash', 'Bash ls', { command: 'ls', description: 'List files' }),
    );
    expect(text).not.toContain('<code>Bash</code>');
  });

  it('falls back to a plain ask when the agent gave no description', () => {
    const text = vnodeToText(renderQuestion('Bash', 'Bash git status', { command: 'git status' }));
    expect(text).toContain('<p><strong>Let the coding agent run this command?</strong></p>');
    expect(text).toContain('<pre><code>git status</code></pre>');
    expect(text).not.toContain('<blockquote>');
  });
});

describe('the permission card shape', () => {
  const cards = {
    tool: renderQuestion('Edit', 'Edit /a.md'),
    shell: renderQuestion('Bash', 'Bash ls', { command: 'ls', description: 'List files' }),
    patch: renderQuestion('file_change', 'file_change', { changes: [{ path: '/a' }, { path: '/b' }], reason: 'Fix' }),
    guard: renderCommandQuestion('rm -rf /x', 'Deletes a folder for good.'),
    mcp: renderMcpQuestion('slack', 'post_message', '{"text":"hi"}'),
  };

  it.each(Object.entries(cards))('the %s card opens with the bold question, in the shared classes', (_, card) => {
    expect(vnodeToText(card)).toMatch(
      /^<div class="question-text markdown-content"><p><strong>Let the [^<]*(<code>[^<]*<\/code>[^<]*)?\?<\/strong><\/p>/,
    );
  });

  it.each(Object.entries(cards))('the %s card draws no copy button', (_, card) => {
    expect(vnodeToText(card)).not.toContain('copy');
  });

  it('puts the guard\'s risk summary as a fact before the command', () => {
    expect(vnodeToText(cards.guard))
      .toContain('</strong></p><p>Deletes a folder for good.</p><pre><code>rm -rf /x</code></pre>');
  });

  it('names an MCP tool and its server, with the inputs as the payload', () => {
    const text = vnodeToText(cards.mcp);
    expect(text).toContain('Let the Lucidos Agent use <code>post_message</code> from slack?');
    expect(text).toContain('<pre><code>{"text":"hi"}</code></pre>');
    expect(vnodeToText(renderMcpQuestion('slack', 'list', '{}'))).not.toContain('<pre>');
  });

  it('heads the reason box with the engine\'s own heading', () => {
    const rust = readFileSync(
      new URL('../../../../../lucidos-core/src/engine/thread_events/permission/request.rs', import.meta.url),
      'utf8',
    );
    expect(rust).toContain(`pub const REASON_HEADING: &str = ${JSON.stringify(REASON_HEADING)};`);
  });
});

describe('sessionLabel', () => {
  it('names a path grant by its basename, so a long path stays compact', () => {
    expect(sessionLabel('Edit(/Users/me/repo/.claude/commands/harden.md)')).toBe('harden.md');
    expect(sessionLabel('NotebookEdit(/tmp/nb.ipynb)')).toBe('nb.ipynb');
    expect(sessionLabel('Edit(README.md)')).toBe('README.md');
  });

  it('names a command grant by its head, so the scope reads per program', () => {
    expect(sessionLabel('Bash(git:*)')).toBe('git …');
    expect(sessionLabel('command_execution(git:*)')).toBe('git …');
    expect(sessionLabel('command_execution(./scripts/check.sh:*)')).toBe('./scripts/check.sh …');
  });

  it('names a skill grant by its plugin glob, and a bare grant as itself', () => {
    expect(sessionLabel('Skill(superpowers:*)')).toBe('superpowers:*');
    expect(sessionLabel('Read')).toBe('Read');
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

  it('keeps picked/rejected styling when answered even if terminated also true', () => {
    expect(permissionButtonState({ answered: true, terminated: true, isPicked: true }))
      .toEqual({ disabled: true, stateClass: ' permission-btn-picked' });
    expect(permissionButtonState({ answered: true, terminated: true, isPicked: false }))
      .toEqual({ disabled: true, stateClass: ' permission-btn-rejected' });
  });
});

describe('resolvedChoice', () => {
  it('maps the user\'s own Deny, or an old reasonless refusal, to "deny"', () => {
    expect(resolvedChoice({ allowed: false, reason: 'User denied' })).toBe('deny');
    expect(resolvedChoice({ allowed: false })).toBe('deny');
  });

  it('marks no choice on a refusal the engine wrote', () => {
    expect(resolvedChoice({ allowed: false, reason: 'Interrupted by an engine restart before anyone answered' })).toBeNull();
    expect(resolvedChoice({ allowed: false, reason: 'Superseded by a new message' })).toBeNull();
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

describe('grantLabel', () => {
  it('reads a command grant as the command, in words', () => {
    expect(vnodeToText(grantLabel('Bash(git:*)'))).toBe('Always allow <code>git</code> commands');
  });
  it('reads a skill grant as the plugin it covers', () => {
    expect(vnodeToText(grantLabel('Skill(meta:*)'))).toBe('Always allow skills from <code>meta</code>');
  });
  it('reads a trusted script grant as its path', () => {
    expect(vnodeToText(grantLabel('Bash(./scripts/check.sh:*)')))
      .toBe('Always allow <code>./scripts/check.sh</code> commands');
  });
  it('keeps a pattern it has no words for, rather than inventing some', () => {
    expect(vnodeToText(grantLabel('Read(/tmp/**)'))).toBe('Always allow <code>Read(/tmp/**)</code>');
  });
});
