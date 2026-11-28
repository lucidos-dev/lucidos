# Software Factory Plan Rule

The archive ships the two plan shadow hooks and their shared policy script.
Build it from a commit with:

```sh
git archive --format=zip --output=software-factory-plan-rule.lucidos-plugin \
  HEAD:plugins/software-factory-plan-rule manifest.toml hooks scripts
```

Install the archive in each rollout workspace, then have the owner arm both
hooks. The event types should be `ChangeProposalChecked` and
`ChangeApplyChecked`. Each hook runs its own bundled copy of the policy.
Collect the engine's `[ChangeHookShadow]` entries for a week. Require
observations from each workspace and each hook, with `matched=true` on
every observation. Count unavailable verdicts as failed comparisons.
