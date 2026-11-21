import { getAgentDeniedCommands, putAgentDeniedCommands } from '../../api/client';
import { AllowlistEditor } from './AllowlistEditor';

export function StandingDeniesSection() {
  return (
    <AllowlistEditor
      title="Standing denies"
      anchor="permissions:standing-denies"
      noun="standing denies"
      rememberButton="Always deny"
      placeholder="Bash(curl:*)"
      load={getAgentDeniedCommands}
      save={putAgentDeniedCommands}
      description={
        <>
          The Lucidos Agent, triggers and coding agents refuse commands that match these rules. A match beats every Always allow.
          Patterns: <code>Bash(&lt;program&gt;:*)</code> e.g. <code>Bash(curl:*)</code>, <code>Bash</code> (any shell), <code>Python</code> (any Python).
          Use <strong>Always deny</strong> in a command prompt's Remember menu to add one quickly.
          A rule matches the program's name. It protects no file, and a script that runs the program inside itself is not stopped.
          If a coding agent asks for approval while the rules can’t be checked, its card explains why and offers Allow once or Deny.
          Commands run independently by provider extensions are outside these checks.
        </>
      }
    />
  );
}
