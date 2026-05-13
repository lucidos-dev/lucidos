import { describe, it, expect } from 'vitest';
import type { ComponentChildren, VNode } from 'preact';
import {
  BROAD_ALLOW_INEFFECTIVE,
  renderQuestion,
  resolvedChoice,
  sessionLabel,
} from '../PermissionCard';

/** Minimal vnode → plain text walker (no DOM, no preact-render-to-string).
 *  The tests just need to assert "the tool name appears as <strong>" and
 *  "the path appears as <code>" so we collect a flat tag-tagged string. */
function vnodeToText(node: ComponentChildren): string {
  if (node === null || node === undefined || typeof node === 'boolean') return '';
  if (typeof node === 'string' || typeof node === 'number') return String(node);
  if (Array.isArray(node)) return node.map(vnodeToText).join('');
  const v = node as VNode<{ children?: ComponentChildren }>;
  const tag = typeof v.type === 'string' ? v.type : '';
  const inner = vnodeToText(v.props?.children);
  return tag ? `<${tag}>${inner}</${tag}>` : inner;
}

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
