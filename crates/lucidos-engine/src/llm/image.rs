use async_trait::async_trait;
use base64::Engine as _;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

use crate::core::PreferenceStore;
use crate::llm::vertex::{LocationHandle, TokenCache};

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
// OpenAI gpt-image-* provider
// ---------------------------------------------------------------------------

pub struct OpenAiImageProvider {
    api_key: String,
    model: String,
    name: String,
    client: reqwest::Client,
}

impl OpenAiImageProvider {
    pub fn new(api_key: String, model: String) -> Self {
        let name = format!("OpenAI {}", model);
        Self {
            api_key,
            model,
            name,
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
                "model": self.model,
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
                .text("model", self.model.clone())
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
        &self.name
    }
}

// ---------------------------------------------------------------------------
// Vertex AI Imagen 4 provider
// ---------------------------------------------------------------------------

pub struct VertexImagenProvider {
    project_id: String,
    location: LocationHandle,
    token_cache: TokenCache,
    client: reqwest::Client,
}

impl VertexImagenProvider {
    pub fn with_location_handle(
        project_id: String,
        location: LocationHandle,
        token_cache: TokenCache,
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

    fn current_location(&self) -> String {
        crate::llm::vertex::read_location(&self.location)
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
        let location = self.current_location();

        let (url, body) = if input_images.is_empty() {
            // Text-to-image generation
            let url = format!(
                "https://{}/v1/projects/{}/locations/{}/publishers/google/models/imagen-4.0-generate-001:predict",
                crate::llm::vertex::vertex_host(&location), self.project_id, location
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
                crate::llm::vertex::vertex_host(&location), self.project_id, location
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

/// Resolve the configured image provider from the live `image_model`
/// preference. Constructed fresh per call so Settings changes take effect
/// without an engine restart; cost is dwarfed by the image-API roundtrip.
pub async fn build_image_provider(
    pool: &sqlx::PgPool,
    openai_api_key: Option<&str>,
    vertex_project_id: &str,
    vertex_location: &LocationHandle,
    vertex_token_cache: &Option<TokenCache>,
) -> Option<Arc<dyn ImageProvider>> {
    let pref = PreferenceStore::get(pool, crate::core::PREF_IMAGE_MODEL)
        .await
        .unwrap_or_else(|e| {
            crate::log!("[Image] Failed to read image_model preference: {}", e);
            None
        });
    let model = pref.as_deref().unwrap_or("auto");

    let build_imagen = || -> Arc<dyn ImageProvider> {
        let tc = vertex_token_cache
            .clone()
            .unwrap_or_else(|| Arc::new(std::sync::Mutex::new(None)));
        Arc::new(VertexImagenProvider::with_location_handle(
            vertex_project_id.to_string(),
            vertex_location.clone(),
            tc,
        ))
    };
    let build_openai = |key: &str, model: &str| -> Arc<dyn ImageProvider> {
        Arc::new(OpenAiImageProvider::new(key.to_string(), model.to_string()))
    };

    match model {
        "gpt-image-1" | "gpt-image-1.5" | "gpt-image-2" => match openai_api_key {
            Some(key) => {
                crate::log!("[Image] Using OpenAI {}", model);
                Some(build_openai(key, model))
            }
            None => {
                crate::log!("[Image] {} selected but OPENAI_API_KEY not set", model);
                None
            }
        },
        "imagen-4" => {
            if vertex_project_id.is_empty() {
                crate::log!("[Image] imagen-4 selected but VERTEX_PROJECT_ID not set");
                return None;
            }
            crate::log!("[Image] Using Vertex AI Imagen 4");
            Some(build_imagen())
        }
        _ => {
            if !vertex_project_id.is_empty() {
                crate::log!("[Image] Auto-selected Vertex AI Imagen 4");
                Some(build_imagen())
            } else if let Some(key) = openai_api_key {
                crate::log!("[Image] Auto-selected OpenAI gpt-image-1");
                Some(build_openai(key, "gpt-image-1"))
            } else {
                crate::log!(
                    "[Image] No image provider available (no Vertex or OpenAI credentials)"
                );
                None
            }
        }
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

    #[tokio::test]
    async fn build_image_provider_reads_current_preference_each_call() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let location = crate::llm::vertex::location_handle("us-central1".into());
        let token_cache: Option<TokenCache> = Some(Arc::new(std::sync::Mutex::new(None)));
        let project_id = "test-project";
        let api_key = Some("sk-test");

        PreferenceStore::set(&pool, crate::core::PREF_IMAGE_MODEL, "imagen-4")
            .await
            .unwrap();
        let p1 =
            build_image_provider(&pool, api_key, project_id, &location, &token_cache).await;
        assert_eq!(
            p1.expect("provider should be built").name(),
            "Vertex AI Imagen 4"
        );

        PreferenceStore::set(&pool, crate::core::PREF_IMAGE_MODEL, "gpt-image-2")
            .await
            .unwrap();
        let p2 =
            build_image_provider(&pool, api_key, project_id, &location, &token_cache).await;
        assert_eq!(
            p2.expect("provider should be built").name(),
            "OpenAI gpt-image-2"
        );

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }

    #[tokio::test]
    async fn build_image_provider_auto_mode_prefers_vertex() {
        let (pool, db_name) = crate::test_support::setup_test_db().await;
        let location = crate::llm::vertex::location_handle("us-central1".into());
        let token_cache: Option<TokenCache> = Some(Arc::new(std::sync::Mutex::new(None)));

        // No preference set → auto. Vertex configured → Imagen.
        let p1 =
            build_image_provider(&pool, Some("sk-test"), "test-project", &location, &token_cache)
                .await;
        assert_eq!(
            p1.expect("provider should be built").name(),
            "Vertex AI Imagen 4"
        );

        // Auto with no Vertex → falls back to OpenAI gpt-image-1.
        let p2 =
            build_image_provider(&pool, Some("sk-test"), "", &location, &token_cache).await;
        assert_eq!(
            p2.expect("provider should be built").name(),
            "OpenAI gpt-image-1"
        );

        // Auto with no credentials at all → None.
        let p3 = build_image_provider(&pool, None, "", &location, &token_cache).await;
        assert!(p3.is_none(), "no credentials → no provider");

        pool.close().await;
        crate::test_support::teardown_test_db(&db_name).await;
    }
}
