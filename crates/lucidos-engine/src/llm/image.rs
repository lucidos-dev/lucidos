use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use std::time::Duration;

/// Image size presets that map to provider-specific dimensions.
#[derive(Debug, Clone, Copy)]
pub enum ImageSize {
    Square,
    Landscape,
    Portrait,
    Auto,
}

impl ImageSize {
    pub fn parse_size(s: &str) -> Self {
        match s {
            "square" => ImageSize::Square,
            "landscape" => ImageSize::Landscape,
            "portrait" => ImageSize::Portrait,
            _ => ImageSize::Auto,
        }
    }
}

/// Result from an image generation/editing call.
pub struct ImageResult {
    /// Raw image bytes (PNG or JPEG).
    pub bytes: Vec<u8>,
    /// MIME type of the image.
    pub mime_type: String,
}

/// Trait for image generation providers.
#[async_trait]
pub trait ImageProvider: Send + Sync {
    /// Generate or edit an image.
    /// - `prompt`: text describing what to generate or how to edit
    /// - `input_images`: optional existing images to edit (as raw bytes)
    /// - `size`: desired output size
    async fn generate(
        &self,
        prompt: &str,
        input_images: Vec<Vec<u8>>,
        size: ImageSize,
    ) -> Result<ImageResult, Box<dyn std::error::Error + Send + Sync>>;

    /// Whether this provider supports multiple input images for editing.
    fn supports_multi_image(&self) -> bool;

    /// Provider display name.
    fn name(&self) -> &str;
}

// ---------------------------------------------------------------------------
// OpenAI gpt-image-1 provider
// ---------------------------------------------------------------------------

pub struct OpenAiImageProvider {
    api_key: String,
    client: reqwest::Client,
}

impl OpenAiImageProvider {
    pub fn new(api_key: String) -> Self {
        Self {
            api_key,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    fn openai_size(size: ImageSize) -> &'static str {
        match size {
            ImageSize::Square => "1024x1024",
            ImageSize::Landscape => "1536x1024",
            ImageSize::Portrait => "1024x1536",
            ImageSize::Auto => "auto",
        }
    }
}

#[derive(Deserialize)]
struct OpenAiImageResponse {
    data: Vec<OpenAiImageData>,
}

#[derive(Deserialize)]
struct OpenAiImageData {
    b64_json: Option<String>,
}

#[async_trait]
impl ImageProvider for OpenAiImageProvider {
    async fn generate(
        &self,
        prompt: &str,
        input_images: Vec<Vec<u8>>,
        size: ImageSize,
    ) -> Result<ImageResult, Box<dyn std::error::Error + Send + Sync>> {
        let size_str = Self::openai_size(size);

        if input_images.is_empty() {
            // Text-to-image generation
            let body = serde_json::json!({
                "model": "gpt-image-1",
                "prompt": prompt,
                "n": 1,
                "size": size_str,
                "output_format": "png",
            });

            let resp = self
                .client
                .post("https://api.openai.com/v1/images/generations")
                .header("Authorization", format!("Bearer {}", self.api_key))
                .header("Content-Type", "application/json")
                .json(&body)
                .send()
                .await?;

            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(
                    format!("OpenAI image generation failed ({}): {}", status, text).into(),
                );
            }

            let response: OpenAiImageResponse = resp.json().await?;
            let b64 = response
                .data
                .first()
                .and_then(|d| d.b64_json.as_ref())
                .ok_or("No image data in OpenAI response")?;

            let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
            Ok(ImageResult {
                bytes,
                mime_type: "image/png".to_string(),
            })
        } else {
            // Image editing with multipart form
            let mut form = reqwest::multipart::Form::new()
                .text("model", "gpt-image-1")
                .text("prompt", prompt.to_string())
                .text("n", "1")
                .text("size", size_str.to_string());

            for (i, img_bytes) in input_images.into_iter().enumerate() {
                let part = reqwest::multipart::Part::bytes(img_bytes)
                    .file_name(format!("image_{}.png", i))
                    .mime_str("image/png")?;
                form = form.part("image[]".to_string(), part);
            }

            let resp = self
                .client
                .post("https://api.openai.com/v1/images/edits")
                .header("Authorization", format!("Bearer {}", self.api_key))
                .multipart(form)
                .send()
                .await?;

            let status = resp.status();
            if !status.is_success() {
                let text = resp.text().await.unwrap_or_default();
                return Err(format!("OpenAI image edit failed ({}): {}", status, text).into());
            }

            let response: OpenAiImageResponse = resp.json().await?;
            let b64 = response
                .data
                .first()
                .and_then(|d| d.b64_json.as_ref())
                .ok_or("No image data in OpenAI edit response")?;

            let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
            Ok(ImageResult {
                bytes,
                mime_type: "image/png".to_string(),
            })
        }
    }

    fn supports_multi_image(&self) -> bool {
        true
    }

    fn name(&self) -> &str {
        "OpenAI gpt-image-1"
    }
}

// ---------------------------------------------------------------------------
// Vertex AI Imagen 4 provider
// ---------------------------------------------------------------------------

pub struct VertexImagenProvider {
    project_id: String,
    location: String,
    token_cache: crate::llm::vertex::TokenCache,
    client: reqwest::Client,
}

impl VertexImagenProvider {
    pub fn new(
        project_id: String,
        location: String,
        token_cache: crate::llm::vertex::TokenCache,
    ) -> Self {
        Self {
            project_id,
            location,
            token_cache,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(120))
                .build()
                .expect("Failed to build HTTP client"),
        }
    }

    fn get_access_token(&self) -> Result<String, Box<dyn std::error::Error + Send + Sync>> {
        crate::llm::vertex::get_cached_access_token(&self.token_cache)
    }

    fn imagen_aspect_ratio(size: ImageSize) -> &'static str {
        match size {
            ImageSize::Square => "1:1",
            ImageSize::Landscape => "16:9",
            ImageSize::Portrait => "9:16",
            ImageSize::Auto => "1:1",
        }
    }
}

#[async_trait]
impl ImageProvider for VertexImagenProvider {
    async fn generate(
        &self,
        prompt: &str,
        input_images: Vec<Vec<u8>>,
        size: ImageSize,
    ) -> Result<ImageResult, Box<dyn std::error::Error + Send + Sync>> {
        let token = self.get_access_token()?;
        let aspect_ratio = Self::imagen_aspect_ratio(size);

        let (url, body) = if input_images.is_empty() {
            // Text-to-image generation
            let url = format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/google/models/imagen-4.0-generate-001:predict",
                crate::llm::vertex::vertex_host(&self.location), self.project_id, self.location
            );
            let body = serde_json::json!({
                "instances": [{"prompt": prompt}],
                "parameters": {
                    "sampleCount": 1,
                    "aspectRatio": aspect_ratio,
                    "outputOptions": {"mimeType": "image/png"}
                }
            });
            (url, body)
        } else {
            // Image editing uses imagen-3.0-capability-001 with REFERENCE_TYPE_RAW
            // (instruct customization). imagen-4.0-generate-001 does not support referenceImages.
            let img_b64 = base64::engine::general_purpose::STANDARD.encode(&input_images[0]);
            let url = format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/google/models/imagen-3.0-capability-001:predict",
                crate::llm::vertex::vertex_host(&self.location), self.project_id, self.location
            );
            let body = serde_json::json!({
                "instances": [{
                    "prompt": prompt,
                    "referenceImages": [{
                        "referenceId": 1,
                        "referenceType": "REFERENCE_TYPE_RAW",
                        "referenceImage": {
                            "bytesBase64Encoded": img_b64
                        }
                    }]
                }],
                "parameters": {
                    "sampleCount": 1,
                    "aspectRatio": aspect_ratio,
                    "outputOptions": {"mimeType": "image/png"}
                }
            });
            (url, body)
        };

        let resp = self
            .client
            .post(&url)
            .header("Authorization", format!("Bearer {}", token))
            .header("Content-Type", "application/json")
            .json(&body)
            .send()
            .await?;

        let status = resp.status();
        if !status.is_success() {
            let text = resp.text().await.unwrap_or_default();
            return Err(format!("Imagen generation failed ({}): {}", status, text).into());
        }

        let response: serde_json::Value = resp.json().await?;
        let b64 = response["predictions"][0]["bytesBase64Encoded"]
            .as_str()
            .ok_or("No image data in Imagen response")?;

        let bytes = base64::engine::general_purpose::STANDARD.decode(b64)?;
        Ok(ImageResult {
            bytes,
            mime_type: "image/png".to_string(),
        })
    }

    fn supports_multi_image(&self) -> bool {
        false
    }

    fn name(&self) -> &str {
        "Vertex AI Imagen 4"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn image_size_parse_size_parses_known_values() {
        assert!(matches!(ImageSize::parse_size("square"), ImageSize::Square));
        assert!(matches!(
            ImageSize::parse_size("landscape"),
            ImageSize::Landscape
        ));
        assert!(matches!(
            ImageSize::parse_size("portrait"),
            ImageSize::Portrait
        ));
        assert!(matches!(ImageSize::parse_size("auto"), ImageSize::Auto));
        assert!(matches!(ImageSize::parse_size("unknown"), ImageSize::Auto));
    }

    #[test]
    fn openai_size_mapping() {
        assert_eq!(
            OpenAiImageProvider::openai_size(ImageSize::Square),
            "1024x1024"
        );
        assert_eq!(
            OpenAiImageProvider::openai_size(ImageSize::Landscape),
            "1536x1024"
        );
        assert_eq!(
            OpenAiImageProvider::openai_size(ImageSize::Portrait),
            "1024x1536"
        );
        assert_eq!(OpenAiImageProvider::openai_size(ImageSize::Auto), "auto");
    }

    #[test]
    fn imagen_aspect_ratio_mapping() {
        assert_eq!(
            VertexImagenProvider::imagen_aspect_ratio(ImageSize::Square),
            "1:1"
        );
        assert_eq!(
            VertexImagenProvider::imagen_aspect_ratio(ImageSize::Landscape),
            "16:9"
        );
        assert_eq!(
            VertexImagenProvider::imagen_aspect_ratio(ImageSize::Portrait),
            "9:16"
        );
    }
}
