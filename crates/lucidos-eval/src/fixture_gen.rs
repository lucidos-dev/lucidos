//! One-shot generator for the 40 seeded memory rows.
//!
//! Behind the `fixture-gen` feature so the ONNX embedder is not a dependency of
//! the crate every contributor builds. Run it when `memory-seed.toml` changes,
//! and commit the rewritten block in `seed.sql`.
//!
//! It embeds exactly the way the engine does: the same model id, the same raw
//! summary text, no query or passage prefix. A vector from a different recipe
//! sits at a different distance from every query. The control arm's recall
//! would then stop resembling a used workspace.

use std::fmt::Write as _;
use std::path::Path;

use fastembed::{InitOptions, TextEmbedding};
use serde::Deserialize;

type Fallible<T> = Result<T, Box<dyn std::error::Error + Send + Sync>>;

/// Marks the region of `seed.sql` this generator owns.
const BEGIN_MARKER: &str = "-- BEGIN GENERATED memory_entries";
const END_MARKER: &str = "-- END GENERATED memory_entries";

/// Commit recorded on every seeded row's artifact source. Literal, because a
/// real sha would make the fixture depend on this repository's history.
const FIXTURE_COMMIT: &str = "0000000000000000000000000000000000000000";

/// Insert time recorded on every seeded row. Literal for the same reason.
const FIXTURE_CREATED_AT: &str = "2026-01-01 00:00:00+00";

#[derive(Debug, Deserialize)]
struct MemorySeedFile {
    embedding_model: String,
    extractor_version: i32,
    entry: Vec<MemorySeedEntry>,
}

#[derive(Debug, Deserialize)]
struct MemorySeedEntry {
    id: String,
    topic: String,
    summary: String,
    importance: f32,
    entities: Vec<String>,
    source_path: String,
    created: String,
}

/// Read the seed file, embed every summary, and rewrite `seed.sql`'s block.
pub fn generate(fixture_dir: &Path) -> Fallible<()> {
    let seed_toml = fixture_dir.join("memory-seed.toml");
    let seed_sql = fixture_dir.join("seed.sql");
    let parsed: MemorySeedFile = toml::from_str(&std::fs::read_to_string(&seed_toml)?)?;

    let model = resolve_model(&parsed.embedding_model)?;
    let options = InitOptions::new(model).with_show_download_progress(true);
    let embedder = TextEmbedding::try_new(options)?;
    let texts: Vec<String> = parsed.entry.iter().map(|e| e.summary.clone()).collect();
    let vectors = embedder.embed(texts, None)?;

    let mut block = String::new();
    block.push_str(BEGIN_MARKER);
    block.push_str(
        "\n-- Written by `cargo run -p lucidos-eval --features fixture-gen -- \
         generate-memory-fixture`\n-- from fixtures/memory-seed.toml. Never edit by hand: the \
         vectors are model\n-- output and the digest depends on them byte for byte.\n",
    );
    block.push_str(
        "INSERT INTO memory_entries \
         (id, source, topic, summary, importance, entities, embedding, embedding_model, \
         src_created_at, created_at, extractor_version) VALUES\n",
    );
    for (index, (entry, vector)) in parsed.entry.iter().zip(vectors.iter()).enumerate() {
        let source = serde_json::json!({
            "type": "artifact",
            "path": entry.source_path,
            "commit": FIXTURE_COMMIT,
        });
        let entities = serde_json::to_string(&entry.entities)?;
        let separator = if index + 1 == parsed.entry.len() {
            ";"
        } else {
            ","
        };
        writeln!(
            block,
            "  ('{}', '{}'::jsonb, '{}', '{}', {}, '{}'::jsonb, '{}'::vector, '{}', \
             '{}', '{}', {}){}",
            entry.id,
            quote(&source.to_string()),
            quote(&entry.topic),
            quote(&entry.summary),
            entry.importance,
            quote(&entities),
            pgvector_literal(vector),
            quote(&parsed.embedding_model),
            entry.created,
            FIXTURE_CREATED_AT,
            parsed.extractor_version,
            separator,
        )?;
    }
    block.push_str(END_MARKER);

    let sql = std::fs::read_to_string(&seed_sql)?;
    std::fs::write(&seed_sql, replace_block(&sql, &block)?)?;
    println!(
        "wrote {} memory rows into {}",
        parsed.entry.len(),
        seed_sql.display()
    );
    Ok(())
}

fn resolve_model(id: &str) -> Fallible<fastembed::EmbeddingModel> {
    match id {
        "multilingual-e5-small" => Ok(fastembed::EmbeddingModel::MultilingualE5Small),
        "bge-small-en-v1.5" => Ok(fastembed::EmbeddingModel::BGESmallENV15),
        other => Err(format!(
            "unknown embedding model {other:?} in memory-seed.toml. It must be one the \
             engine can also load, or recall would compare vectors from two models."
        )
        .into()),
    }
}

/// Escape a value for a single-quoted SQL literal.
fn quote(value: &str) -> String {
    value.replace('\'', "''")
}

fn pgvector_literal(vector: &[f32]) -> String {
    let mut out = String::with_capacity(vector.len() * 12);
    out.push('[');
    for (index, value) in vector.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        let _ = write!(out, "{value}");
    }
    out.push(']');
    out
}

/// Swap the marked region for `block`, refusing when the markers are gone.
fn replace_block(sql: &str, block: &str) -> Fallible<String> {
    let start = sql.find(BEGIN_MARKER).ok_or_else(missing_marker)?;
    let end_start = sql.find(END_MARKER).ok_or_else(missing_marker)?;
    let end = end_start + END_MARKER.len();
    if end_start < start {
        return Err(missing_marker());
    }
    Ok(format!("{}{}{}", &sql[..start], block, &sql[end..]))
}

fn missing_marker() -> Box<dyn std::error::Error + Send + Sync> {
    format!(
        "seed.sql has no `{BEGIN_MARKER}` / `{END_MARKER}` pair, so there is nowhere to write \
         the generated rows. Restore the markers rather than pasting rows by hand."
    )
    .into()
}
