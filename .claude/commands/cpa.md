Commit and push all changes in the lucidos repo. Follow these steps exactly:

1. Run `git status -s` in the lucidos repo root to see all changes.

2. **Check for unpushed commits**: Run `git log @{u}..HEAD --oneline` to see if there are commits that haven't been pushed yet. These may be from other sessions.

3. **Workspace changes check**: If any changed files are under a workspace directory (e.g., `~/workspaces/test/`, `~/workspaces/personal/`, or any path containing `/workspaces/`), ALERT the user about them but do NOT stage or commit them. List the workspace files and say "Workspace changes detected — not committed."

4. **Other sessions check**: If on the main working tree (not a worktree), run `pgrep -af 'claude'` to check for other Claude processes. If other sessions are running, STOP and warn:
   - "⚠️ Other Claude sessions are active. Uncommitted changes may belong to another session's in-progress work."
   - List the dirty files from step 1
   - Ask the user to confirm before proceeding: "Commit these files? (list them explicitly so the user can say which to include/exclude)"
   - If the user declines or excludes files, only stage/commit the approved files

5. **Lucidos repo changes**: If there are uncommitted changes (from step 1):
   - Run `git diff` to review what's being committed
   - Run `git log --oneline -3` for commit message style
   - Stage all non-workspace changed files by name (never use `git add -A` or `git add .`)
   - Write a conventional commit message (`feat:`, `fix:`, `refactor:`, `docs:`, `chore:`, etc.) summarizing the changes
   - Commit

6. **Current worktree only**: If your current working directory is inside a git worktree (not the main working tree), commit and push that worktree's branch:
   - Run `git status -s` to check for uncommitted changes
   - If there are changes: stage them by name, commit with a conventional commit message
   - Push the worktree branch to the remote: `git push -u origin <branch-name>`
   - **Do NOT touch other worktrees.** Each session is responsible for its own worktree only. Never cd into, commit in, or push branches from worktrees you are not working in — they belong to other sessions (terminal Claude or Lucidos).

7. **Push**: Push all commits on the current branch (both new and previously unpushed) to the remote.

8. If there are no changes to commit anywhere AND no unpushed commits, say so.

9. **Summary**: After completing, print a detailed summary:
   - Commit hash(es), message(s), branch, and remote
   - If there were **previously unpushed commits** (from other sessions), list them separately with their hashes and messages
   - A **numbered breakdown of every logical change** across all pushed commits. Group related file changes together by what they accomplish. For each group, give a short bolded title and 1-2 sentences explaining what changed and why. Example:

     **1. Web search: DuckDuckGo → Gemini grounding** (`vertex.rs`, `engine.rs`, `Cargo.toml`)
     Added `search_with_grounding()` using Gemini's google_search tool. Replaced the 100-line DuckDuckGo HTML scraper.

     **2. Streaming race condition fix** (`api.rs`)
     Moved `subscribe()` before spawning workers so events aren't missed.

   This helps the user understand what they just shipped, not just which files changed.
