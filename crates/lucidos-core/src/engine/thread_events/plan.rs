use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanApprovalKind {
    Design,
    Implementation,
    SimpleFix,
    BoundedSecurityFix,
}

impl From<PlanKind> for PlanApprovalKind {
    fn from(kind: PlanKind) -> Self {
        match kind {
            PlanKind::Design => Self::Design,
            PlanKind::Implementation => Self::Implementation,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PlanScope {
    Implement,
    PlanOnly,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ChangeContent {
    #[default]
    Code,
    PlanOnly,
}

impl ChangeContent {
    pub fn from_files(files: &[String]) -> Self {
        if crate::core::git::lands_plan_files_only(files) {
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
