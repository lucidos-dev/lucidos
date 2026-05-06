#!/bin/bash
# Download PaddleOCR Latin models for Norwegian/European language support
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"

# Default to test-space-2 or use LUCIDOS_WORKSPACE
WORKSPACE="${LUCIDOS_WORKSPACE:-$PROJECT_DIR/test-space-2}"
MODELS_DIR="$WORKSPACE/.lucidos/models/ocr"

echo "Downloading OCR models to: $MODELS_DIR"
mkdir -p "$MODELS_DIR"
cd "$MODELS_DIR"

# Base URL for model downloads (from rust-paddle-ocr next branch)
BASE_URL="https://raw.githubusercontent.com/zibo-chen/rust-paddle-ocr/next/models"

# Required models for Latin (Norwegian, English, European languages)
echo "Downloading detection model..."
curl -L -o "PP-OCRv5_mobile_det.mnn" "$BASE_URL/PP-OCRv5_mobile_det.mnn"

echo "Downloading Latin recognition model..."
curl -L -o "latin_PP-OCRv5_mobile_rec.mnn" "$BASE_URL/latin_PP-OCRv5_mobile_rec_infer.mnn"

echo "Downloading Latin character keys..."
curl -L -o "ppocr_keys_latin.txt" "$BASE_URL/ppocr_keys_latin.txt"

echo ""
echo "OCR models downloaded successfully!"
echo "Location: $MODELS_DIR"
ls -lh "$MODELS_DIR"
