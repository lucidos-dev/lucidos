use std::path::{Path, PathBuf};

/// Safely extract text from a PDF, catching panics from malformed PDFs.
/// The pdf-extract crate uses .unwrap() internally on some parse results,
/// which can panic on invalid content streams.
pub(super) fn safe_extract_pdf_text(path: &Path) -> Result<String, String> {
    let path = path.to_path_buf();
    match std::panic::catch_unwind(move || pdf_extract::extract_text(&path)) {
        Ok(Ok(text)) => Ok(text),
        Ok(Err(e)) => Err(format!("PDF extract error: {}", e)),
        Err(_) => Err("PDF extract panicked on malformed content".to_string()),
    }
}

/// Extract text from a scanned PDF using PaddleOCR (ocr-rs)
/// Requires poppler-utils for PDF to image conversion
pub(super) fn extract_text_with_ocr(pdf_path: &Path) -> Result<String, String> {
    use std::process::Command;

    // Create temp directory for images
    let temp_dir = std::env::temp_dir().join(format!("lucidos_ocr_{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&temp_dir).map_err(|e| format!("Failed to create temp dir: {}", e))?;

    // Convert PDF pages to images using pdftoppm (from poppler-utils)
    // Using 300 DPI for better OCR quality
    let pdf_path_str = pdf_path.to_string_lossy();
    let output_prefix = temp_dir.join("page");

    let pdftoppm_result = Command::new("pdftoppm")
        .args([
            "-png",
            "-r",
            "300",
            &pdf_path_str,
            output_prefix.to_string_lossy().as_ref(),
        ])
        .output();

    if let Err(e) = pdftoppm_result {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err(format!(
            "pdftoppm not available (install poppler-utils): {}",
            e
        ));
    }

    // Find all generated page images
    let mut page_images: Vec<_> = std::fs::read_dir(&temp_dir)
        .map_err(|e| format!("Failed to read temp dir: {}", e))?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "png"))
        .map(|e| e.path())
        .collect();

    if page_images.is_empty() {
        let _ = std::fs::remove_dir_all(&temp_dir);
        return Err("pdftoppm produced no images".to_string());
    }

    page_images.sort();
    let total_pages = page_images.len();

    // OCR each page with PaddleOCR (ocr-rs)
    let mut all_text = String::new();

    for (i, image_path) in page_images.iter().enumerate() {
        let page_num = i + 1;

        // Add page header
        if total_pages > 1 {
            if i > 0 {
                all_text.push_str("\n\n");
            }
            all_text.push_str(&format!("=== Side {} av {} ===\n\n", page_num, total_pages));
        }

        // Load and OCR the image
        match ocr_image_with_paddle(image_path) {
            Ok(text) => {
                if !text.trim().is_empty() {
                    all_text.push_str(&text);
                } else {
                    all_text.push_str("[Ingen tekst gjenkjent på denne siden]");
                }
            }
            Err(e) => {
                log!(@OCR, "Failed to OCR page {}: {}", page_num, e);
                all_text.push_str(&format!("[OCR feilet: {}]", e));
            }
        }
    }

    // Cleanup
    let _ = std::fs::remove_dir_all(&temp_dir);

    if all_text.trim().is_empty() {
        return Err("OCR produced no text".to_string());
    }

    Ok(all_text)
}

/// OCR a single image using PaddleOCR (ocr-rs)
pub(super) fn ocr_image_with_paddle(image_path: &Path) -> Result<String, String> {
    use ocr_rs::OcrEngine;

    // Get models directory (in .lucidos/models/)
    let models_dir = std::env::var("LUCIDOS_WORKSPACE")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
        .join(".lucidos/models/ocr");

    // Model paths (Latin model supports European languages)
    let det_model = models_dir.join("PP-OCRv5_mobile_det.mnn");
    let rec_model = models_dir.join("latin_PP-OCRv5_mobile_rec.mnn"); // Downloaded as latin_PP-OCRv5_mobile_rec_infer.mnn
    let keys_file = models_dir.join("ppocr_keys_latin.txt");

    // Check if models exist
    if !det_model.exists() || !rec_model.exists() || !keys_file.exists() {
        return Err(format!(
            "OCR models not found. Please download Latin models to {:?}\n\
             Download from: https://github.com/zibo-chen/rust-paddle-ocr/releases",
            models_dir
        ));
    }

    // Load image
    let img = image::open(image_path).map_err(|e| format!("Failed to load image: {}", e))?;

    // Create OCR engine
    let engine = OcrEngine::new(
        det_model.to_string_lossy().as_ref(),
        rec_model.to_string_lossy().as_ref(),
        keys_file.to_string_lossy().as_ref(),
        None, // No classifier model needed for Latin
    )
    .map_err(|e| format!("Failed to create OCR engine: {}", e))?;

    // Run OCR
    let results = engine
        .recognize(&img)
        .map_err(|e| format!("OCR failed: {}", e))?;

    // Combine all detected text blocks, sorted by vertical position
    let mut text_blocks: Vec<(i32, String)> = results
        .iter()
        .map(|r| (r.bbox.rect.top(), r.text.clone())) // (y-position, text)
        .collect();

    text_blocks.sort_by_key(|(y, _)| *y);

    let text: String = text_blocks
        .into_iter()
        .map(|(_, t)| t)
        .collect::<Vec<_>>()
        .join("\n");

    Ok(text)
}
