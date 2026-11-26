pub(crate) const MAX_BULK_FILE_COUNT: usize = 500;
pub(crate) const MAX_BULK_BYTES: u64 = 100 * 1024 * 1024;

pub(crate) enum BulkContext<'a> {
    Clone { url: &'a str },
    ImportFile { source_path: &'a str },
    HttpResponse { dest: &'a str },
}

impl BulkContext<'_> {
    fn noun(&self) -> String {
        match self {
            BulkContext::Clone { url } => format!("clone of {}", url),
            BulkContext::ImportFile { source_path } => (*source_path).to_string(),
            BulkContext::HttpResponse { dest } => format!("HTTP response → {}", dest),
        }
    }
}

pub(crate) fn bulk_threshold_error(
    ctx: BulkContext,
    file_count: Option<usize>,
    bytes: u64,
) -> String {
    let size_mb = bytes as f64 / (1024.0 * 1024.0);
    let count_part = file_count
        .map(|n| format!("{} files, ", n))
        .unwrap_or_default();
    format!(
        "Error: refusing to write {} ({}{:.1} MB) to data/artifacts/. \
         Bulk imports >{} files or >100 MB bloat workspace artifact count and slow the UI \
         (see system-knowhow/best-practices rule 8). Choose one: \
         (a) extract only the subset the app uses and discard the rest; \
         (b) move the dataset to ~/.lucidos/data/<name>/ (use `lucidos data-store add <name> <source-dir>`) \
         and pin the absolute path in the relevant app's knowhow; \
         (c) for research / inspect-only clones, target `.lucidos/tmp/<name>/` instead.",
        ctx.noun(),
        count_part,
        size_mb,
        MAX_BULK_FILE_COUNT
    )
}
