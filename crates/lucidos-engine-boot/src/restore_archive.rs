use std::path::PathBuf;

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    use std::io::Write;

    let mut file: Option<PathBuf> = None;
    let mut workspace_dir: Option<PathBuf> = None;
    let mut it = std::env::args().skip(2);
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--file" => file = it.next().map(PathBuf::from),
            "--workspace-dir" => workspace_dir = it.next().map(PathBuf::from),
            other => {
                return Err(format!("restore-archive: unexpected argument '{other}'").into());
            }
        }
    }
    let file = file.ok_or("restore-archive: --file is required")?;
    let workspace_dir = workspace_dir.ok_or("restore-archive: --workspace-dir is required")?;

    let _ = dotenvy::dotenv();
    let key_text = std::env::var("LUCIDOS_RESTORE_KEY")
        .map_err(|_| "restore-archive: LUCIDOS_RESTORE_KEY env var is required")?;
    let key: lucidos_core::core::backup::crypto::OpeningKey = key_text
        .parse()
        .map_err(|e| format!("restore-archive: {e}"))?;
    let database_url = lucidos_core::core::database_url();

    let warnings = lucidos_core::core::backup::restore_archive_into(
        &workspace_dir,
        &database_url,
        key,
        &file,
        |phase, cur, _total| {
            let mut out = std::io::stdout().lock();
            let _ = writeln!(out, "LUCIDOS_RESTORE_PHASE={phase}:{cur}");
            let _ = out.flush();
        },
    )
    .await?;
    let mut out = std::io::stdout().lock();
    for warning in warnings {
        writeln!(
            out,
            "LUCIDOS_RESTORE_WARNING={}",
            warning.replace('\n', " ")
        )?;
    }
    out.flush()?;
    Ok(())
}
