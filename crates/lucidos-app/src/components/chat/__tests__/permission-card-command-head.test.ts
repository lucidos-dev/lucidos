import { describe, it, expect } from 'vitest';
import { commandGrantHead, commandHead } from '../PermissionCard';

/** `commandHead` labels the permission card's narrow and session buttons, and
 *  the engine derives the pattern it actually STORES from the same command.
 *  The two must agree. Otherwise the button names one grant and the click
 *  persists another.
 *
 *  The engine side is `first_command_token` -> `segment_heads` ->
 *  `unwrap_shell_command` in `engine/command_guard.rs`. These cases mirror that
 *  function's own tests. */
describe('commandHead mirrors the engine grant derivation', () => {
  it('reads the head of a bare command', () => {
    expect(commandHead('curl -X POST https://example.com')).toBe('curl');
  });

  it('takes the basename of an absolute path', () => {
    expect(commandHead('/usr/bin/rsync -a a b')).toBe('rsync');
  });

  it('skips privilege and wrapper prefixes', () => {
    expect(commandHead('sudo env FOO=1 apt-get install x')).toBe('apt-get');
  });

  it('returns null for an empty command', () => {
    expect(commandHead('   ')).toBeNull();
  });

  // The reason this file exists. Codex sends every command pre-wrapped, so the
  // wrapped shape is the ordinary path. Reading the raw text gave all of these
  // the head `bash`. A narrow grant then read `Bash(bash:*)` on the card while
  // the engine stored the inner head.
  it.each([
    ["bash -lc 'curl -X POST https://api.example.com/pay'", 'curl'],
    ['bash -c "npm install"', 'npm'],
    ["/bin/zsh -lc 'git status'", 'git'],
    ["sh -c 'rm -rf build'", 'rm'],
    ["dash -c 'ls'", 'ls'],
    ["bash -lc '/usr/local/bin/deploy.sh'", 'deploy.sh'],
    ["bash -lc 'sudo apt-get update'", 'apt-get'],
  ])('unwraps %s to the inner head', (command, expected) => {
    expect(commandHead(command)).toBe(expected);
  });

  // The engine keeps the WHOLE command when the tail after the script operand
  // could run something. That tail belongs to the outer shell. The card has to
  // fall back in the same cases.
  it.each([
    ["bash -lc 'git status' > /tmp/out"],
    ["bash -lc 'git status'; rm -rf /"],
    ["bash -lc 'git status' | tee log"],
    ['bash -lc "git status" && curl evil'],
    ["bash -lc 'git status' $(whoami)"],
  ])('names no head when the tail can run more: %s', (command) => {
    expect(commandHead(command)).toBeNull();
  });

  // A shell head runs its arguments, so the engine stores no grant for it and
  // the card offers none.
  it('names no head when there is no -c flag', () => {
    expect(commandHead('bash deploy.sh')).toBeNull();
    expect(commandHead('\\bash deploy.sh')).toBeNull();
  });

  it('reads an unquoted script operand whole, as the engine does', () => {
    // The engine returns an unquoted operand with its tail attached, so the
    // head comes from the script. Returning the wrapper here labelled the card
    // `bash` while the click stored `Bash(ls:*)`.
    expect(commandHead('bash -c ls')).toBe('ls');
  });

  it('does not treat a long option as the -c flag', () => {
    expect(commandHead("bash --command 'curl x'")).toBeNull();
  });

  // A shell word JOINS adjacent quoted runs, and `'\''` is the POSIX idiom for
  // a literal quote inside a single-quoted string. Cutting at the first close
  // quote read `bash -c 'rm -rf '\''/'\'''` as `rm -rf `.
  it.each([
    [String.raw`bash -c 'rm -rf '\''/'\'''`, 'rm'],
    [String.raw`bash -c 'echo -n '\''/'\'''`, 'echo'],
    [String.raw`/bin/zsh -lc 'git '\''status'\'''`, 'git'],
    [String.raw`bash -c "rm -rf "'/'`, 'rm'],
  ])('joins adjacent quoted runs in %s', (command, expected) => {
    expect(commandHead(command)).toBe(expected);
  });

  it('skips a leading redirect rather than reading it as the command', () => {
    expect(commandHead('2>data/log rm -rf x')).toBe('rm');
    expect(commandHead('> out.txt ls -la')).toBe('ls');
  });

  // The engine walks every token looking for the `-c` cluster. Testing only
  // the first dash token gave up here and labelled the card `bash`, while the
  // click stored `Bash(rm:*)`.
  it.each([
    ["bash -o pipefail -c 'rm -rf /'", 'rm'],
    ["bash -l -c 'git status'", 'git'],
    ["/bin/zsh -i -lc 'cargo test'", 'cargo'],
  ])('finds the -c cluster past an earlier flag in %s', (command, expected) => {
    expect(commandHead(command)).toBe(expected);
  });

  it('names no head when a substitution is glued to the operand', () => {
    // The opener has to land in the TAIL, or the whole substitution vanishes.
    expect(commandHead("bash -c 'echo hi'$(rm -rf /)")).toBeNull();
    expect(commandHead("bash -c 'echo hi'`rm -rf /`")).toBeNull();
    // A plain `$VAR` is not a substitution and must not truncate the script.
    expect(commandHead("bash -c 'ls '$HOME")).toBe('ls');
  });

  // Codex sometimes nests a second shell inside its own wrapper. The engine
  // reads through every layer, so the card names the real command.
  it.each([
    [`/bin/zsh -lc "bash -c 'git push'"`, 'git'],
    [`/bin/zsh -lc "bash -c 'sh -c ls'"`, 'ls'],
  ])('reads through the nested shell in %s', (command, expected) => {
    expect(commandHead(command)).toBe(expected);
  });

  it.each([
    ['timeout 5 curl https://example.com/x'],
    ['eval ls'],
    ['source x.sh'],
    ['. x.sh'],
    [`/bin/zsh -lc "bash x.sh"`],
  ])('names no head for a command that runs its arguments: %s', (command) => {
    expect(commandHead(command)).toBeNull();
  });
});

/** Only the engine knows the workspace root, so only it can drop a leading
 *  `cd <root> &&`. The card names the head the engine sends. */
describe('commandGrantHead', () => {
  const card = { request_id: 'r', tool_use_id: 't', summary: 's' };

  it('prefers the head the engine sent', () => {
    expect(commandGrantHead({
      ...card,
      tool_name: 'run_bash',
      command: 'cd /Users/me/ws && python3 x.py',
      grant_head: 'python3',
    })).toBe('python3');
  });

  it('reads the command on a card from before the field', () => {
    expect(commandGrantHead({ ...card, tool_name: 'run_bash', command: 'git push' })).toBe('git');
  });

  it('names no head for python', () => {
    expect(commandGrantHead({ ...card, tool_name: 'run_python', command: 'print(1)' })).toBeNull();
  });
});
