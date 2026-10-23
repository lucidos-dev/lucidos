/** A change's commit subjects, oldest first: what the change card and the
 *  Changes panel unfold under a change's headline. */
export function CommitList({ commits }: { commits: readonly string[] }) {
  return (
    <ol class="change-commit-list" data-role="change-commit-list">
      {commits.map((subject, i) => <li key={i}>{subject}</li>)}
    </ol>
  );
}
