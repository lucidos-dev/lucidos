use serde::{Deserialize, Serialize};

/// The purpose of a plan, supplied explicitly when planning starts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanKind {
    Design,
    #[default]
    Implementation,
}

impl PlanKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Design => "design",
            Self::Implementation => "implementation",
        }
    }
}

/// What the user's plan approval permits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanScope {
    Implement,
    PlanOnly,
}

/// Content of the actual proposed or landed diff. Historical events default to code.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeContent {
    #[default]
    Code,
    PlanOnly,
}

impl ChangeContent {
    pub(crate) fn from_files(files: &[String]) -> Self {
        if crate::engine::git_ops::lands_plan_files_only(files) {
            Self::PlanOnly
        } else {
            Self::Code
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ChangeContent;

    #[test]
    fn content_requires_a_nonempty_exclusively_plan_diff() {
        assert_eq!(ChangeContent::from_files(&[]), ChangeContent::Code);
        assert_eq!(
            ChangeContent::from_files(&["docs/plans/design.md".into()]),
            ChangeContent::PlanOnly
        );
        assert_eq!(
            ChangeContent::from_files(&["docs/plans/design.md".into(), "src/main.rs".into()]),
            ChangeContent::Code
        );
    }
}
