include!("shared_fixtures.rs");

pub(super) fn per_commit_proposed(
    branch: &str,
    sha: &str,
    subject: &str,
    requires_restart: bool,
) -> ThreadEvent {
    ThreadEvent::ChangeProposed {
        content: Default::default(),
        change_id: String::new(),
        description: Some(subject.to_string()),
        files: vec!["c.rs".to_string()],
        requires_restart,
        origin: None,
        commit_sha: Some(sha.to_string()),
        branch_name: branch.to_string(),
        repo_root: String::new(),
        hardened: false,
        incomplete: false,
        set_aside: false,
        path: String::new(),
        diff: String::new(),
    }
}

pub(super) fn discarded_event(change_id: Uuid) -> ThreadEvent {
    ThreadEvent::ChangeDiscarded {
        change_id: change_id.to_string(),
        actor: None,
        path: String::new(),
    }
}
