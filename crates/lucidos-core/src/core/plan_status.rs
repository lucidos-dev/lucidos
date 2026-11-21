/// The status line the `implementation-plan` skill writes into a new plan.
const PROPOSED_STATUS: &str = "- **Status**: Proposed";
/// What `planned approve` rewrites it to.
pub const APPROVED_STATUS: &str = "- **Status**: Approved";

/// Recognize the status line the implementation-plan skill writes.
pub fn is_plan_status_line(line: &str) -> bool {
    matches!(line.trim_end(), PROPOSED_STATUS | APPROVED_STATUS)
}

/// Flip the first status line if it is proposed.
pub fn approved_plan_text(text: &str) -> Option<String> {
    let mut found = false;
    let mut flipped = false;
    let out: String = text
        .split_inclusive('\n')
        .map(|line| {
            if !found && is_plan_status_line(line) {
                found = true;
                if line.trim_end() == PROPOSED_STATUS {
                    flipped = true;
                    return line.replacen(PROPOSED_STATUS, APPROVED_STATUS, 1);
                }
            }
            line.to_string()
        })
        .collect();
    flipped.then_some(out)
}
