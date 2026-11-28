use crate::core::apps::WidgetSize;
use crate::core::App;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetAudience {
    Chat { automatic: bool },
    CodingAgent,
}

pub fn build_widgets_section(widgets: &[App], audience: WidgetAudience) -> String {
    format!("{}{}", widgets_rules(audience), widgets_list(widgets))
}

pub fn widgets_rules(audience: WidgetAudience) -> String {
    match audience {
        WidgetAudience::Chat { automatic } => format!(
            "\n\n## Widgets\n\n{}{EMBED_RULE}",
            making_guidance(automatic)
        ),
        WidgetAudience::CodingAgent => {
            format!("\n\n## Widget embeds\n\n{EMBED_RULE}{CODING_AGENT_FIT_RULE}")
        }
    }
}

fn making_guidance(automatic: bool) -> String {
    let mut text = String::from(
        "A widget is a small interactive answer: create_app with kind=\"widget\", then embed it \
         in your reply. It draws into a fixed box of its size, the same in its thread card and \
         on a dashboard: style under html[data-widget-size] and fit by showing less, never by \
         shrinking text or scrolling inside. More than a large box holds is an app. Name it by \
         what it does in two or three words (\"Flight picker\"), and put the specifics inside it. ",
    );
    text.push_str(if automatic {
        "Choose one yourself for a comparison, or for a pick among data items such as \
         flights, times or products: never list those as question options. Answer in text \
         when the user asks for text. "
    } else {
        "Automatic widgets are off: make a widget only when the user asks for one. For a pick \
         among data items, offer one as a question option. Embedding a listed widget is not \
         making one. "
    });
    text.push_str(
        "Lead with one shape, not a table: a .ring, a .bar-run, or \
         a .choice-row to pick from. Draw no border inside the widget. Keep the accent for the \
         one thing that matters. Fix every cut a [WIDGET FIT] note names; never ellipsize data \
         text. Animate what \
         a tap changes, on the --duration-* tokens, and stay calm under data-motion=\"reduce\". \
         Read data through lucidos.* and listen on lucidos.sse instead of baking it into the \
         HTML. It takes small, medium and large unless create_app sizes changes them or adds \
         xl. Arrange a dashboard or resize a card only when the user asks in this turn, and \
         never choose a size yourself.\n\n",
    );
    text
}

const EMBED_RULE: &str = "EMBED A WIDGET WHERE A PICTURE DRAWS: `![label](app:<id>?params={...})` \
     draws it live in an option's description or preview, a card's message, or a reply. Only \
     the leading `!` draws it. The alt text is its label, and params are a JSON object of its \
     listed names. Embed one when the user must hear, see or try something to decide, such as \
     a voice sample or a video clip in each option. Put an option's sample only in it, never above the card. \
     One embed per option. A bad embed refuses the card: fix it and ask again.\n";

const CODING_AGENT_FIT_RULE: &str = "After you change a widget's files, run `lucidos widgets fit \
     --app-id <id>` and fix every cut it names.\n";

pub const WIDGETS_LIST_HEADER: &str = "\nWidgets to embed:\n";

pub fn widgets_list(widgets: &[App]) -> String {
    if widgets.is_empty() {
        return String::new();
    }
    let mut text = String::from(WIDGETS_LIST_HEADER);
    for widget in widgets {
        text.push_str(&widget_line(widget));
    }
    text
}

pub fn widget_line(widget: &App) -> String {
    let built_in = if widget.built_in { ", built in" } else { "" };
    let mut declared = widget.sizes.clone();
    declared.sort();
    let mut undeclared = WidgetSize::UNDECLARED.to_vec();
    undeclared.sort();
    let sizes = if declared != undeclared {
        let names: Vec<&str> = widget.sizes.iter().map(|s| s.as_str()).collect();
        format!(", sizes {}", names.join(", "))
    } else {
        String::new()
    };
    let mut text = format!(
        "- **{}** (id: `{}`{built_in}{sizes}): {}\n",
        widget.name,
        widget.id,
        app_description(&widget.description)
    );
    for (name, param) in &widget.params {
        let required = if param.required { " (required)" } else { "" };
        text.push_str(&format!(
            "  - param `{name}`{required}: {}\n",
            app_description(&param.description)
        ));
    }
    text
}

pub const APP_DESCRIPTION_MAX_CHARS: usize = 200;

/// INVARIANT: Cut `text` to at most `max_chars` characters on a word boundary and mark
/// the cut with an ellipsis.
///
/// Use the boundary only in the last quarter of the budget. This still
/// truncates a long unbroken URL or base64 string. Count characters to preserve
/// UTF-8 boundaries.
pub fn truncate_on_word_boundary(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let head: String = text.chars().take(max_chars).collect();
    let cut = match head.rfind(char::is_whitespace) {
        Some(idx) if head[..idx].chars().count() >= max_chars * 3 / 4 => idx,
        _ => head.len(),
    };
    format!("{}…", head[..cut].trim_end())
}

pub fn app_description(description: &str) -> String {
    truncate_on_word_boundary(description, APP_DESCRIPTION_MAX_CHARS)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::apps::AppParam;
    use crate::core::{AppKind, AppReveal};

    fn player() -> App {
        App {
            id: "lucidos-sound-player".into(),
            name: "Sound player".into(),
            description: "Plays one sound clip.".into(),
            icon: None,
            reveal: AppReveal::OnLoad,
            kind: AppKind::Widget,
            origin: None,
            reusable: true,
            params: std::collections::BTreeMap::from([(
                "clip".to_string(),
                AppParam {
                    description: "A workspace path or an https URL".into(),
                    required: true,
                },
            )]),
            built_in: true,
            sizes: WidgetSize::UNDECLARED.to_vec(),
            transparent: false,
        }
    }

    #[test]
    fn the_chat_and_coding_agent_sections_share_the_embed_guidance() {
        let widgets = [player()];
        let shared = format!("{EMBED_RULE}{}", widgets_list(&widgets));
        for automatic in [true, false] {
            let chat = build_widgets_section(&widgets, WidgetAudience::Chat { automatic });
            assert!(chat.ends_with(&shared), "{chat}");
        }
        let coding = build_widgets_section(&widgets, WidgetAudience::CodingAgent);
        let coding_tail = format!(
            "{EMBED_RULE}{CODING_AGENT_FIT_RULE}{}",
            widgets_list(&widgets)
        );
        assert!(coding.ends_with(&coding_tail), "{coding}");
        assert!(
            !coding.contains("create_app"),
            "a coding agent makes no widget: {coding}"
        );
    }

    #[test]
    fn the_embed_guidance_names_the_form_the_when_and_each_param() {
        let text = build_widgets_section(&[player()], WidgetAudience::CodingAgent);
        for rule in [
            "`![label](app:<id>?params={...})`",
            "Only the leading `!` draws it",
            "a voice sample or a video clip in each option",
            "Put an option's sample only in it, never above the card",
            "A bad embed refuses the card",
            "- **Sound player** (id: `lucidos-sound-player`, built in): Plays one sound clip.",
            "  - param `clip` (required): A workspace path or an https URL",
        ] {
            assert!(text.contains(rule), "missing {rule:?}: {text}");
        }
    }

    #[test]
    fn the_list_names_sizes_only_for_a_widget_that_changes_them() {
        assert!(!widget_line(&player()).contains("sizes"));
        let clock = App {
            sizes: vec![WidgetSize::Small, WidgetSize::Large],
            ..player()
        };
        assert!(widget_line(&clock).contains("built in, sizes small, large):"));
        let film = App {
            sizes: vec![WidgetSize::Medium, WidgetSize::Large, WidgetSize::Xl],
            ..player()
        };
        assert!(widget_line(&film).contains("built in, sizes medium, large, xl):"));
    }

    #[test]
    fn the_chat_agent_sizes_nothing_unasked_and_pins_nothing() {
        for automatic in [true, false] {
            let chat = build_widgets_section(&[player()], WidgetAudience::Chat { automatic });
            assert!(chat.contains("resize a card only when the user asks in this turn"));
            assert!(chat.contains("never choose a size yourself"));
            assert!(chat.contains("fixed box of its size"));
            assert!(chat.contains("never by shrinking text"));
            assert!(!chat.contains("pin") && !chat.contains("shelf"), "{chat}");
        }
    }

    #[test]
    fn both_agents_call_this_one_builder() {
        let chat = include_str!(
            "../../../lucidos-agents-api/src/engine/chat/process/workspace_prompt_footprint.rs"
        );
        assert!(chat.contains("crate::engine::widget_guidance::build_widgets_section("));
        assert!(chat.contains("WidgetAudience::Chat { automatic }"));
        let spawn = include_str!(
            "../../../lucidos-agents-api/src/engine/agent_session/run_session/spawn_context.rs"
        );
        assert!(spawn.contains("crate::engine::widget_guidance::build_widgets_section("));
        assert!(spawn.contains("WidgetAudience::CodingAgent"));
        assert!(spawn.contains("self.coding_agent_widgets()"));
    }

    #[test]
    fn the_chat_agent_learns_the_widget_look() {
        let coding = build_widgets_section(&[player()], WidgetAudience::CodingAgent);
        for automatic in [true, false] {
            let chat = build_widgets_section(&[player()], WidgetAudience::Chat { automatic });
            assert!(chat.contains("one shape, not a table"), "{chat}");
            assert!(chat.contains("no border inside the widget"), "{chat}");
            for class in [".ring", ".bar-run", ".choice-row"] {
                assert!(chat.contains(class), "{class}: {chat}");
                assert!(!coding.contains(class), "{class}: {coding}");
            }
        }
    }

    #[test]
    fn both_agents_learn_the_widget_fit_check() {
        let coding = build_widgets_section(&[player()], WidgetAudience::CodingAgent);
        assert!(coding.contains("lucidos widgets fit --app-id"), "{coding}");
        for automatic in [true, false] {
            let chat = build_widgets_section(&[player()], WidgetAudience::Chat { automatic });
            assert!(chat.contains("[WIDGET FIT]"), "{chat}");
            assert!(chat.contains("never ellipsize data text"), "{chat}");
            assert!(!chat.contains("bar shows the name"), "{chat}");
        }
    }

    #[test]
    fn automatic_widgets_off_still_embeds() {
        let off = build_widgets_section(&[player()], WidgetAudience::Chat { automatic: false });
        assert!(
            off.contains("Embedding a listed widget is not making one"),
            "{off}"
        );
        assert!(off.contains("EMBED A WIDGET"), "{off}");
    }
}
