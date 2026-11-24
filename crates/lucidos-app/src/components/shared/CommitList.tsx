export function CommitList({ commits }: { commits: readonly string[] }) {
  return (
    <ol class="change-commit-list" data-role="change-commit-list">
      {commits.map((subject, i) => <li key={i}>{subject}</li>)}
    </ol>
  );
}
