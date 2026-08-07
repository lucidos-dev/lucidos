//! LLM-facing schemas for image tools (save_thread_image, generate_image,
//! view_image).

use crate::llm::provider::ToolDefinition;
use crate::llm::tool_names as tn;
use serde_json::json;

/// Tool for pulling an image posted earlier in the thread back into vision.
///
/// Recently-posted images are already in the model's vision; older ones age out
/// of the auto-included window and only survive in the history as a text note +
/// description. This tool re-loads any thread image's actual pixels on demand so
/// the model can look at it again ("the image I posted earlier").
pub fn get_view_image_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::VIEW_IMAGE.to_string(),
        description: "Load an image posted earlier in this thread back into your vision so you can SEE its pixels again. Older images drop out of view after a few messages, leaving only a text note, so call this whenever the user refers to one you can no longer see, then answer from what you see. For an image file under data/artifacts/, use read_file.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "Thread image reference 'thread:N', 1-based as shown in the conversation history."
                }
            },
            "required": ["image"]
        }),
    }
}

/// Tool for saving a thread image to an artifact path.
pub fn get_save_thread_image_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::SAVE_THREAD_IMAGE.to_string(),
        description: "Save a conversation image to an artifact file, committed to git, when the user wants to keep one.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "Thread image reference 'thread:N', 1-based across the conversation."
                },
                "path": {
                    "type": "string",
                    "description": "Destination relative to data/artifacts/ (e.g. 'projects/reports/photo.jpg'). Committed to git."
                }
            },
            "required": ["image", "path"]
        }),
    }
}

/// Tool for generating or editing images.
pub fn get_image_generation_tool() -> ToolDefinition {
    ToolDefinition {
        name: tn::GENERATE_IMAGE.to_string(),
        description: "SYNTHESIZES a new image, or edits an existing one. Returns image bytes, never text. \
            NOT a vision or analysis tool: never call it to describe, analyze, summarize or transcribe an \
            image. You can see recent conversation images natively, so describe them directly in your reply; \
            for an older one, call view_image('thread:N') first. \
            `prompt` describes the output image; add `input_images` to edit an existing one. The current \
            provider may accept only one input image, and passing more then fails with an error asking the \
            user to pick.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "The image to synthesize, or how to edit input_images. Must describe a desired output picture, never an instruction like 'describe this image'."
                },
                "input_images": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Images to edit, each 'thread:N' (1-based) or an artifact path. Omit for text-to-image."
                },
                "size": {
                    "type": "string",
                    "enum": ["square", "landscape", "portrait", "auto"],
                    "description": "Output dimensions. Default 'auto'."
                },
                "save_as_artifact": {
                    "type": "string",
                    "description": "Optional path relative to data/artifacts/ to save it (e.g. 'generated/logo.png'). Git-committed."
                }
            },
            "required": ["prompt"]
        }),
    }
}
