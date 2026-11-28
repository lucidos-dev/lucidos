use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CheckpointProgress {
    pub done: u32,
    pub total: u32,
}

impl CheckpointProgress {
    pub fn parse(raw: &str) -> Result<Self, String> {
        let parsed = raw.split_once('/').and_then(|(done, total)| {
            Some((done.trim().parse().ok()?, total.trim().parse().ok()?))
        });
        match parsed {
            Some((done, total)) if total >= 1 && done <= total => Ok(Self { done, total }),
            _ => Err(format!(
                "The progress reads `<done>/<total>`, with 1 <= total and done <= total. Got `{raw}`."
            )),
        }
    }

    pub fn units_left(&self) -> u32 {
        self.total - self.done
    }
}

impl std::fmt::Display for CheckpointProgress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.done, self.total)
    }
}
