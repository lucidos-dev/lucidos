//! One-shot snapshot commands invoked by the gateway.

use lucidos_core::core::snapshot::offline;

pub use offline::Arguments;

pub enum Command {
    Restore,
    Recover,
}

impl Command {
    pub async fn run(
        self,
        args: Arguments,
        database_url: String,
    ) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
        match self {
            Self::Restore => offline::run(args, database_url).await,
            Self::Recover => offline::recover(args, database_url).await,
        }
    }
}
