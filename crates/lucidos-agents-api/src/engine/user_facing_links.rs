/// Shared by the Lucidos Agent and both coding-agent backends.
pub(super) const USER_FACING_LINKS: &str = r#"LINK WHAT YOU NAME:
Link named items in replies, summaries, notifications and question text. Use visible names and tool links. Keep IDs, SHAs and branches out of prose and option labels unless requested or needed to copy.
- App: `[Name](app:<id>)`. Add `#fragment` for an item the app opens via `location.hash`.
- Thread: `[Title](thread:<workspace>/<id>)`.
- Workspace file, artifact or knowhow: link its data-relative path, e.g. `artifacts/notes.md`.
- Repo file: `[Path](repo:<registered-repo-id-or-name>:file:<path>)`. For revisions use `repo:<repo>:file#<ref>:<path>`.
- Trigger: `[Name](trigger:<id>)`. Get the ID from the trigger list. Run once is on its row. The Triggers panel opens the list.
- Commit or change: use a verified URL or link its thread or file. Label by commit subject or originating thread and scope.
For lines in files, append `#L5-L9` or `#L5`. Wrap targets with spaces in angle brackets."#;

#[cfg(test)]
mod tests {
    use super::USER_FACING_LINKS;

    #[test]
    fn user_facing_links_preserve_clickable_forms_and_visible_labels() {
        for required in [
            "[Name](app:<id>)",
            "`#fragment`",
            "`location.hash`",
            "[Title](thread:<workspace>/<id>)",
            "[Name](trigger:<id>)",
            "Run once is on its row",
            "repo:<repo>:file#<ref>:<path>",
            "question text",
            "Keep IDs, SHAs and branches out of prose and option labels",
        ] {
            assert!(USER_FACING_LINKS.contains(required), "missing {required}");
        }
    }
}
