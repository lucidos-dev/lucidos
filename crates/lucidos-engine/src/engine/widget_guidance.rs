//! The widgets section an agent's prompt carries (ADR 0415).
//!
//! One builder serves the chat agent and the coding agents, so both learn the
//! same embed rule and see the same widgets. The chat agent also makes and
//! shows widgets, so its section opens with that guidance.

use crate::core::App;

/// Who reads the section.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WidgetAudience {
    /// The chat agent, which makes widgets. `automatic` is the
    /// `automatic_widgets` preference.
    Chat { automatic: bool },
    /// A coding agent, which embeds them in its question options and replies.
    CodingAgent,
}

/// The widgets section for `audience`, listing `widgets`: every built-in and
/// reusable widget, each with its params.
pub fn build_widgets_section(widgets: &[App], audience: WidgetAudience) -> String {
    format!("{}{}", widgets_rules(audience), widgets_list(widgets))
}

/// The section's rules, sent whether or not the workspace keeps a widget. The
/// chat agent's are *system prompt footprint* (ADR 0413), metered by
/// `always_loaded_context_stays_under_budget` with the built-in widgets'
/// lines. A coding agent's ride its spawn-time prompt, as the memory and
/// literacy sections do.
pub fn widgets_rules(audience: WidgetAudience) -> String {
    match audience {
        WidgetAudience::Chat { automatic } => format!(
            "\n\n## Widgets\n\n{}{EMBED_RULE}",
            making_guidance(automatic)
        ),
        WidgetAudience::CodingAgent => format!("\n\n## Widget embeds\n\n{EMBED_RULE}"),
    }
}

/// How the chat agent makes, shows and pins a widget (ADRs 0402, 0407).
fn making_guidance(automatic: bool) -> String {
    let mut text = String::from(
        "A widget is a small interactive answer: create_app with kind=\"widget\". It shows \
         inline at this turn as a card, and is not on the shelf unless pinned. Fit one phone \
         screen with no scroll area inside, so its top screen carries the answer; more than \
         that is an app. Name it by what it does in two or three words (\"Flight picker\"), \
         and put the specifics inside it. ",
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
        "Pin it with widgets(action=\"pin\") only when the user will come back to it in this \
         thread, such as a picker. Animate what a tap changes, on the --duration-* tokens, and \
         stay calm under data-motion=\"reduce\". Read data through lucidos.* and listen on \
         lucidos.sse instead of baking it into the HTML. widgets(action=\"show\", params=…) \
         puts a listed widget in this thread as a card.\n\n",
    );
    text
}

/// The embed rule, identical for every audience.
const EMBED_RULE: &str = "EMBED A WIDGET WHERE A PICTURE DRAWS: `![label](app:<id>?params={...})` \
     draws it live in an option's description or preview, a card's message, or a reply. Only \
     the leading `!` draws it. The alt text is its label, and params are a JSON object of its \
     listed names. Embed one when the user must hear, see or try something to decide, such as \
     a voice sample in each option. Never put links above a card for the user to match to its \
     options. One embed per option. A bad embed refuses the card: fix it and ask again.\n";

/// The widgets an agent may show or embed, each with its params. Empty when
/// there is none.
pub fn widgets_list(widgets: &[App]) -> String {
    if widgets.is_empty() {
        return String::new();
    }
    let mut text = String::from("\nWidgets to embed:\n");
    for widget in widgets {
        let built_in = if widget.built_in { ", built in" } else { "" };
        text.push_str(&format!(
            "- **{}** (id: `{}`{built_in}): {}\n",
            widget.name,
            widget.id,
            app_description(&widget.description)
        ));
        for (name, param) in &widget.params {
            let required = if param.required { " (required)" } else { "" };
            text.push_str(&format!(
                "  - param `{name}`{required}: {}\n",
                app_description(&param.description)
            ));
        }
    }
    text
}

fn app_description(text: &str) -> String {
    crate::engine::chat::process::workspace_prompt_footprint::app_description(text)
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
            origin_thread_id: None,
            reusable: true,
            params: std::collections::BTreeMap::from([(
                "clip".to_string(),
                AppParam {
                    description: "A workspace path or an https URL".into(),
                    required: true,
                },
            )]),
            built_in: true,
        }
    }

    /// Both agents learn one embed rule and see one list (plan invariant:
    /// one builder).
    #[test]
    fn the_chat_and_coding_agent_sections_share_the_embed_guidance() {
        let widgets = [player()];
        let shared = format!("{EMBED_RULE}{}", widgets_list(&widgets));
        for automatic in [true, false] {
            let chat = build_widgets_section(&widgets, WidgetAudience::Chat { automatic });
            assert!(chat.ends_with(&shared), "{chat}");
        }
        let coding = build_widgets_section(&widgets, WidgetAudience::CodingAgent);
        assert!(coding.ends_with(&shared), "{coding}");
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
            "a voice sample in each option",
            "A bad embed refuses the card",
            "- **Sound player** (id: `lucidos-sound-player`, built in): Plays one sound clip.",
            "  - param `clip` (required): A workspace path or an https URL",
        ] {
            assert!(text.contains(rule), "missing {rule:?}: {text}");
        }
    }

    /// Both prompts build the section here: the chat agent through its apps
    /// section, a coding agent at spawn. Pinned in the source, since neither
    /// prompt can be built without an engine.
    #[test]
    fn both_agents_call_this_one_builder() {
        let chat = include_str!("chat/process/workspace_prompt_footprint.rs");
        assert!(chat.contains("crate::engine::widget_guidance::build_widgets_section("));
        assert!(chat.contains("WidgetAudience::Chat { automatic }"));
        let spawn = include_str!("agent_session/run_session/spawn_context.rs");
        assert!(spawn.contains("crate::engine::widget_guidance::build_widgets_section("));
        assert!(spawn.contains("WidgetAudience::CodingAgent"));
        assert!(spawn.contains("self.coding_agent_widgets()"));
    }

    /// Automatic widgets off stops the agent making one, never embedding one.
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
