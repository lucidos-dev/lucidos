//! Simple commands as the shell runs them, each with the heredoc text it reads.
//!
//! The filesystem floor reads operands from these. A quote, a comment or a
//! heredoc body never starts a command here, so data stays data.
use super::operations::bare_head;
use crate::engine::command_guard as guard;

/// What a heredoc body is to the command that reads it.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum Input {
    /// A shell, a pipeline or an unknown program may run it.
    Shell(String),
    /// A Python interpreter runs it.
    Python(String),
    /// Earlier shell code can change which program reads this input.
    Uncertain(String),
    /// A sink copies it and never runs it. `written` when the copy lands in a
    /// file, which a later command may run.
    Data { body: String, written: bool },
}

#[derive(Debug, PartialEq, Eq)]
pub(super) struct LiveSegment {
    pub(super) text: String,
    /// The next segment reads this one's output.
    pub(super) piped: bool,
    pub(super) backgrounded: bool,
    pub(super) conditional: bool,
    pub(super) inputs: Vec<Input>,
}

/// Each simple command in `command`, split only at unquoted control
/// operators. `None` when a quote, an expansion or a heredoc cannot be
/// placed for sure, so the caller falls back to the plain split.
///
/// A substitution body stays inside its segment: the catastrophic floor
/// classifies each body on its own.
pub(super) fn live_segments(command: &str) -> Option<Vec<LiveSegment>> {
    let mut quotes = guard::QuoteState::default();
    let mut segments: Vec<LiveSegment> = Vec::new();
    let mut pending: Vec<(guard::Heredoc, usize)> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let mut word_start = true;
    let mut comment_at = None;
    let mut first_shadow = None;
    while i < command.len() {
        if let Some(next) = quotes.step_all_quotes(command, i) {
            if !command[i..].starts_with("\\\n") {
                word_start = false;
            }
            i = next;
            continue;
        }
        if let Some(end) = guard::expansion_end(command, i) {
            let region = command.get(i..end)?;
            if region.contains("<<") || region.contains('\n') {
                return None;
            }
            word_start = false;
            i = end;
            continue;
        }
        if ["$(", "<(", ">("]
            .iter()
            .any(|s| command[i..].starts_with(s))
            || command.as_bytes()[i] == b'`'
        {
            let span = guard::substitution_spans(&command[i..])
                .into_iter()
                .next()?;
            word_start = false;
            i += span.whole.end;
            continue;
        }
        let byte = command.as_bytes()[i];
        if command[i..].starts_with("&>") || command[i..].starts_with("<&") {
            word_start = false;
            i += 2;
            continue;
        }
        if byte == b'>' {
            i += 1;
            if command[i..].starts_with('>') {
                i += 1;
            }
            if command[i..].starts_with(['|', '!', '&']) {
                i += 1;
            }
            word_start = false;
            continue;
        }
        if byte == b'#' && word_start {
            comment_at = Some(i);
            i = command[i..].find('\n').map_or(command.len(), |n| i + n);
            continue;
        }
        if command[i..].starts_with("<<<") {
            word_start = true;
            i += 3;
            continue;
        }
        if command[i..].starts_with("<<") {
            let (doc, end) = guard::heredoc_opener(command, i + 2)?;
            pending.push((doc, segments.len()));
            word_start = false;
            i = end;
            continue;
        }
        if b";|&\n".contains(&byte) {
            let conditional = command[i..].starts_with("||") || command[i..].starts_with("&&");
            let len = if conditional || command[i..].starts_with("|&") {
                2
            } else {
                1
            };
            let text = &command[start..comment_at.take().unwrap_or(i)];
            if first_shadow.is_none() && can_shadow_command(text) {
                first_shadow = Some(segments.len());
            }
            segments.push(LiveSegment {
                text: text.to_string(),
                piped: byte == b'|' && !conditional,
                backgrounded: byte == b'&' && !conditional,
                conditional,
                inputs: Vec::new(),
            });
            i += len;
            start = i;
            word_start = true;
            if byte == b'\n' && !pending.is_empty() {
                for (doc, consumer) in pending.drain(..) {
                    let (body_end, end) = guard::heredoc_body_end(command, i, &doc)?;
                    let body = &command[i..body_end];
                    let mut input = heredoc_input(&segments[consumer], body);
                    if matches!(input, Input::Data { .. })
                        && first_shadow.is_some_and(|shadow| shadow < consumer)
                    {
                        input = Input::Uncertain(body.to_string());
                    }
                    segments[consumer].inputs.push(input);
                    // An unquoted body runs its substitutions whoever reads it.
                    if !doc.literal {
                        segments[consumer].inputs.extend(
                            guard::substitution_bodies(body)
                                .into_iter()
                                .map(|s| Input::Shell(s.to_string())),
                        );
                    }
                    i = end;
                }
                start = i;
            }
            continue;
        }
        word_start = b" \t\r\n()".contains(&byte);
        i += guard::char_len_at(command, i);
    }
    if quotes.quoted() || !pending.is_empty() {
        return None;
    }
    segments.push(LiveSegment {
        text: command[start..comment_at.unwrap_or(command.len())].to_string(),
        piped: false,
        backgrounded: false,
        conditional: false,
        inputs: Vec::new(),
    });
    Some(segments)
}

pub(super) fn can_shadow_command(command: &str) -> bool {
    static FUNCTION: std::sync::LazyLock<regex::Regex> =
        std::sync::LazyLock::new(|| regex::Regex::new(r"^\s*[^\s(]+\s*\(\s*\)").unwrap());
    FUNCTION.is_match(command)
        || command.split_whitespace().next().is_some_and(|head| {
            matches!(
                head,
                "function" | "alias" | "hash" | "enable" | "source" | "."
            )
        })
}

/// Any consumer this cannot prove to be a sink or Python reads the body as
/// shell, which is how the floor read every body before.
fn heredoc_input(consumer: &LiveSegment, body: &str) -> Input {
    let shell = || Input::Shell(body.to_string());
    if consumer.piped {
        return shell();
    }
    let words = guard::shell_argv(&consumer.text);
    let refs: Vec<_> = words.iter().map(|w| w.text.as_str()).collect();
    let candidates = guard::danger_head_candidates(&refs);
    if candidates.is_empty()
        || candidates
            .iter()
            .any(|(head, _)| guard::GUARD_SHELLS.contains(&head.as_str()))
    {
        return shell();
    }
    if candidates
        .iter()
        .any(|(head, _)| head.starts_with("python"))
    {
        return Input::Python(body.to_string());
    }
    let sink = |head: &str, args: &[&str]| {
        bare_head(&refs, args, head)
            && (guard::DATA_SINK_HEADS.contains(&head)
                || (head == "git" && matches!(args.first(), Some(&("commit" | "tag" | "notes")))))
    };
    if !candidates.iter().all(|(head, args)| sink(head, args)) {
        return shell();
    }
    let tees_to_file = candidates
        .iter()
        .any(|(head, args)| head == "tee" && args.iter().any(|a| !a.starts_with('-')));
    let redirects_to_file = guard::redirect_targets(&consumer.text)
        .any(|target| target != "/dev/null" && !target.starts_with('&'));
    Input::Data {
        body: body.to_string(),
        written: tees_to_file || redirects_to_file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn texts(command: &str) -> Vec<String> {
        live_segments(command)
            .unwrap()
            .into_iter()
            .map(|s| s.text.trim().to_string())
            .collect()
    }

    #[test]
    fn quotes_comments_and_bodies_start_no_command() {
        assert_eq!(
            texts("echo 'a; rm -rf ~' && ls"),
            ["echo 'a; rm -rf ~'", "ls"]
        );
        assert_eq!(texts("echo \"$(a; b)\" | wc"), ["echo \"$(a; b)\"", "wc"]);
        assert_eq!(texts("# a; rm -rf ~\nls 2>&1 >|out"), ["", "ls 2>&1 >|out"]);
        let segments = live_segments("cat > notes.md <<'EOF'\nrm -rf ~\nEOF\nls").unwrap();
        assert_eq!(segments.len(), 2);
        assert_eq!(
            segments[0].inputs,
            [Input::Data {
                body: "rm -rf ~\n".into(),
                written: true
            }]
        );
        assert_eq!(segments[1].text, "ls");
    }

    #[test]
    fn a_body_belongs_to_the_command_that_reads_it() {
        let input = |command: &str| {
            live_segments(command)
                .unwrap()
                .into_iter()
                .flat_map(|s| s.inputs)
                .next()
                .unwrap()
        };
        let body = || "x\n".to_string();
        assert_eq!(input("sh <<'E'\nx\nE"), Input::Shell(body()));
        assert_eq!(input("cat <<'E' | sh\nx\nE"), Input::Shell(body()));
        assert_eq!(input("env ./cat <<'E'\nx\nE"), Input::Shell(body()));
        assert_eq!(input("node <<'E'\nx\nE"), Input::Shell(body()));
        assert_eq!(input("python3 - <<'E'\nx\nE"), Input::Python(body()));
        assert_eq!(
            input("git commit -F - <<'E'\nx\nE"),
            Input::Data {
                body: body(),
                written: false
            }
        );
        assert_eq!(
            input("git -c alias.x='!sh' x <<'E'\nx\nE"),
            Input::Shell(body())
        );
        assert_eq!(
            input("tee -a notes.md <<'E'\nx\nE"),
            Input::Data {
                body: body(),
                written: true
            }
        );
        assert_eq!(
            input("cat <<'E' >/dev/null\nx\nE"),
            Input::Data {
                body: body(),
                written: false
            }
        );
    }

    #[test]
    fn unplaceable_text_has_no_live_segments() {
        assert_eq!(live_segments("echo 'open"), None);
        assert_eq!(live_segments("cat <<'E'"), None);
        assert_eq!(live_segments("echo ${x\n}"), None);
    }
}
