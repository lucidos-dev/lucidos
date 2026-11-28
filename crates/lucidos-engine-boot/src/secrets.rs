use lucidos_core::core::database_url;
use lucidos_core::core::secret_store::{boot, key_store, SecretKey};
use lucidos_core::log_legacy;

pub async fn run() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args: Vec<String> = std::env::args().skip(2).collect();
    let _ = dotenvy::dotenv();
    let database_url = database_url();
    let database = boot::database_name(&database_url)?;
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(1)
        .connect(&database_url)
        .await?;
    let store = key_store::resolve()?;
    match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["import-key", "--key-file", path] => {
            let key = SecretKey::from_base64(&std::fs::read_to_string(path)?)?;
            boot::import_key(&pool, &database, store.as_ref(), &key).await?;
            log_legacy!("[SecretStore] Installed key {} for {database}.", key.id());
        }
        ["discard"] => {
            let removed = boot::discard(&pool, &database, store.as_ref()).await?;
            if removed.is_empty() {
                log_legacy!("[SecretStore] Nothing to discard: every secret in {database} opens.");
            }
            for line in removed {
                log_legacy!("[SecretStore] Discarded {line}");
            }
        }
        _ => {
            return Err(
                "usage: lucidos-engine secrets import-key --key-file <file> \
                        | lucidos-engine secrets discard"
                    .into(),
            )
        }
    }
    Ok(())
}
