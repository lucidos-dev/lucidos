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
        description: "Load an image posted earlier in this thread back into your vision so you can actually SEE its pixels again. Use this whenever the user refers to an image you can no longer see — older images drop out of your view after a few messages, leaving only a text note like '[attached image (thread:2) — image not included]'. Call view_image to bring it back, then describe/answer from what you see. This is for re-viewing conversation images; use read_file for image files saved under data/artifacts/.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "Thread image reference: 'thread:N' where N is the 1-based sequential index shown in the conversation history (same numbering as generate_image's input_images and save_thread_image)."
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
        description: "Save an image from the conversation history to an artifact file. Use this when the user wants to keep an image they pasted or that was generated earlier. The image is committed to git automatically.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "image": {
                    "type": "string",
                    "description": "Thread image reference: 'thread:N' where N is the 1-based sequential index of images in the conversation (same numbering as generate_image's input_images)."
                },
                "path": {
                    "type": "string",
                    "description": "Destination path relative to data/artifacts/ (e.g., 'projects/allergi/photo.jpg'). The image is committed to git."
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
        description: "SYNTHESIZES a new image, or edits an existing image. Returns image bytes — never text. \
            This is NOT a vision/analysis tool: do NOT call it to 'describe', 'analyze', 'summarize', \
            'transcribe', or 'tell me what's in' an image. To describe an image already in the conversation, \
            just describe it directly in your reply — you can see recent ones natively; for an older image \
            you can no longer see, call view_image('thread:N') first, then describe it. \
            Provide `prompt` describing the desired output image. To edit an existing image, also pass \
            `input_images`. The current image provider may only support one input image — if you provide \
            multiple and it's not supported, the call fails with an error asking the user to pick one.".to_string(),
        parameters: json!({
            "type": "object",
            "properties": {
                "prompt": {
                    "type": "string",
                    "description": "Describes the image to be synthesized (or how to edit input_images). Must describe a desired output picture, NOT instructions like 'describe this image' — that wastes a generation call and returns a meaningless image."
                },
                "input_images": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional image references to edit. Each entry is either 'thread:N' (Nth image in the conversation, 1-based) or an artifact path (e.g., 'artifacts/photo.png'). Omit for text-to-image generation."
                },
                "size": {
                    "type": "string",
                    "enum": ["square", "landscape", "portrait", "auto"],
                    "description": "Output image dimensions. Default: 'auto'."
                },
                "save_as_artifact": {
                    "type": "string",
                    "description": "Optional path relative to data/artifacts/ to save the generated image (e.g., 'generated/logo.png'). Image is git-committed."
                }
            },
            "required": ["prompt"]
        }),
    }
}
