CREATE TABLE hardened_branches (
    repo_root TEXT NOT NULL,
    branch_name TEXT NOT NULL,
    head_sha TEXT NOT NULL,
    hardened_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
    PRIMARY KEY (repo_root, branch_name)
);
