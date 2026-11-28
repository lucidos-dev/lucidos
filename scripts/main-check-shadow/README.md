# Main check shadow reader

The Software Factory's script trigger calls `reader.py` from a Lucidos
source checkout. It compares the completed workflow's shard reports with the
engine verdict for the linked run. The engine still owns dispatch and repair.

Supply the same start time and state directory on each fire. The reader
compares checks that started at or after that time. Use the trigger's state
directory for a workspace trigger, or its runtime artifacts directory for a
plugin.

```sh
python3 "$LUCIDOS_SOURCE/scripts/main-check-shadow/reader.py" \
  --since "$MAIN_CHECK_SHADOW_SINCE" --state-dir "$MAIN_CHECK_SHADOW_STATE"
```

The script needs `lucidos`, GitHub CLI and jq. Set `GH_ACCOUNT_GITHUB_COM` to
the named GitHub account with access to the mirror's Actions artifacts.
Keep that account in the trigger's configuration. The script uses the
existing e2e credential resolver.

Subscribe the same trigger to `FactoryMainCheckWorkflowCompleted` with
`actor.kind` equal to `webhook` and `payload.action` equal to `completed`.
Use a dedicated GitHub repository webhook signed with `X-Hub-Signature-256`.
On delivery the reader considers only the matching engine-linked run and
verifies it through the GitHub API. The schedule catches up if delivery
arrives before the engine settles. Each receipt has a `source` of `schedule`
or `webhook` according to which fire recorded it.

Schedule it often enough to read artifacts before they expire after one day.
Use the same state directory for all fires so only one reader emits at a
time. A recorded observation serves as the receipt after a restart. If
GitHub confirms no artifacts remain, the reader records an unavailable
verdict with `matches: null`. A failed download with artifacts still available
leaves the run eligible for the next poll. A validation failure does the same.

The reader skips a verified run if GitHub reports it is still pending. The
scan continues, and the reader checks the run again on a later fire. An
identity mismatch or a change in the workflow attempt during report
collection still fails the scan.

Review `FactoryMainCheckObserved` over the shadow window. Investigate
disagreements, unavailable verdicts and failed fires before cutover. An engine
check that could not link a GitHub run has no hosted artifact to compare.
