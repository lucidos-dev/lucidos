use super::*;
use crate::llm::anthropic_wire::{build_claude_request, WireTarget};
use crate::llm::provider::{ContentBlock, Message, MessageContent, ToolDefinition};

const URL: &str = "https://aiplatform.googleapis.com/v1/projects/p/locations/global\
                   /publishers/anthropic/models/m:streamRawPredict";

fn tools(names: &[&str]) -> Vec<ToolDefinition> {
    names
        .iter()
        .map(|name| ToolDefinition {
            name: name.to_string(),
            description: format!("{name} a file"),
            parameters: serde_json::json!({"type": "object"}),
        })
        .collect()
}

/// A turn's request, the way the chat loop sends it.
struct Turn {
    view: &'static str,
    request_text: &'static str,
    rounds: usize,
    effort: &'static str,
    system: &'static str,
    tools: &'static [&'static str],
}

impl Default for Turn {
    fn default() -> Self {
        Self {
            view: "[THREAD MEMORY VIEW]\n[0+8] old\n",
            request_text: "Request: hello",
            rounds: 1,
            effort: "high",
            system: "system prompt body",
            tools: &["read_file", "write_file"],
        }
    }
}

impl Turn {
    fn messages(&self, tree: bool) -> Vec<Message> {
        let mut blocks = Vec::new();
        if tree {
            blocks.push(ContentBlock::MemoryView {
                text: "[WORKSPACE MEMORY VIEW]\n[w/0+4] shared\n".into(),
            });
            blocks.push(ContentBlock::MemoryView {
                text: self.view.into(),
            });
            blocks.push(ContentBlock::Text {
                text: "[8+1] new\n[END THREAD MEMORY VIEW]".into(),
            });
        }
        blocks.push(ContentBlock::Text {
            text: self.request_text.into(),
        });
        let mut messages = vec![Message {
            role: "user".into(),
            content: MessageContent::Blocks(blocks),
        }];
        for _ in 1..self.rounds {
            messages.push(Message {
                role: "assistant".into(),
                content: MessageContent::Text("reading the file now".into()),
            });
            messages.push(Message {
                role: "user".into(),
                content: MessageContent::Text("[tool result]".into()),
            });
        }
        messages
    }

    fn fingerprint(&self, tree: bool) -> String {
        let (request, _) = build_claude_request(
            self.messages(tree),
            tools(self.tools),
            "claude-opus-5-5",
            Some(self.system),
            Some(self.effort),
            WireTarget::Vertex { url: URL },
            "Vertex",
        );
        fingerprint(&request, "claude-opus-5-5").expect("the prefix serializes")
    }
}

/// Two requests with the same stable prefix hash the same, on both layouts.
#[test]
fn identical_prefixes_share_a_fingerprint() {
    for tree in [false, true] {
        assert_eq!(
            Turn::default().fingerprint(tree),
            Turn::default().fingerprint(tree),
            "tree={tree}"
        );
    }
}

/// A capability gate that adds or drops a tool changes the prefix.
#[test]
fn a_changed_tool_list_changes_the_fingerprint() {
    for tree in [false, true] {
        let fewer = Turn {
            tools: &["read_file"],
            ..Turn::default()
        };
        assert_ne!(
            Turn::default().fingerprint(tree),
            fewer.fingerprint(tree),
            "tree={tree}"
        );
    }
}

/// The next turn rewrites the per-turn block, and the stable prefix ends
/// before it. So two turns that differ only there share a fingerprint.
#[test]
fn the_per_turn_block_is_outside_the_prefix() {
    for tree in [false, true] {
        let next = Turn {
            request_text: "Request: something else entirely",
            ..Turn::default()
        };
        assert_eq!(
            Turn::default().fingerprint(tree),
            next.fingerprint(tree),
            "tree={tree}"
        );
    }
}

/// Round 2 spends its markers on the two newest messages and drops the
/// system marker. The content is the same, so the fingerprint is too.
#[test]
fn the_markers_are_not_part_of_the_fingerprint() {
    for tree in [false, true] {
        let later = Turn {
            rounds: 2,
            ..Turn::default()
        };
        assert_eq!(
            Turn::default().fingerprint(tree),
            later.fingerprint(tree),
            "tree={tree}"
        );
    }
}

#[test]
fn a_changed_memory_view_changes_the_fingerprint() {
    let rolled_over = Turn {
        view: "[THREAD MEMORY VIEW]\n[0+9] rolled over\n",
        ..Turn::default()
    };
    assert_ne!(
        Turn::default().fingerprint(true),
        rolled_over.fingerprint(true)
    );
}

/// Opus 5.5 renders effort into the prefix, so a beat at another effort
/// would read nothing the turn wrote.
#[test]
fn a_changed_effort_changes_the_fingerprint() {
    let medium = Turn {
        effort: "medium",
        ..Turn::default()
    };
    assert_ne!(
        Turn::default().fingerprint(false),
        medium.fingerprint(false)
    );
}

#[test]
fn a_changed_system_block_changes_the_fingerprint() {
    let other = Turn {
        system: "another system prompt",
        ..Turn::default()
    };
    assert_ne!(Turn::default().fingerprint(false), other.fingerprint(false));
}

/// Marking a bare string rewrites it into a one-block array, so the two
/// shapes must read as the same content.
#[test]
fn a_marked_string_reads_as_the_same_content_as_an_unmarked_one() {
    let text = Value::String("hello".into());
    let marked = serde_json::json!([
        {"type": "text", "text": "hello", "cache_control": {"type": "ephemeral"}}
    ]);
    assert_eq!(
        serde_json::to_vec(&Content::of(&text)).unwrap(),
        serde_json::to_vec(&Content::of(&marked)).unwrap()
    );
}
