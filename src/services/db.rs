use mongodb::bson::doc;
use mongodb::options::IndexOptions;
use mongodb::{Client, Collection, Database, IndexModel};
use tracing::{info, warn};

use crate::config::DatabaseConfig;
use crate::models::BucketCredentials;

pub const BUCKETS: &str = "buckets";

pub fn buckets(db: &Database) -> Collection<BucketCredentials> {
    db.collection(BUCKETS)
}

pub async fn connect(
    config: &DatabaseConfig,
) -> Result<Database, Box<dyn std::error::Error + Send + Sync>> {
    let client = Client::with_uri_str(&config.uri).await?;
    let db = client.database(&config.name);
    db.run_command(doc! {"ping": 1}, None).await?;
    info!("Connected to MongoDB database {}", config.name);

    let unique_name = IndexModel::builder()
        .keys(doc! {"name": 1})
        .options(IndexOptions::builder().unique(true).build())
        .build();
    if let Err(e) = buckets(&db).create_index(unique_name, None).await {
        warn!("Could not ensure unique index on bucket name: {}", e);
    }

    Ok(db)
}

pub fn is_duplicate_key(e: &mongodb::error::Error) -> bool {
    use mongodb::error::{ErrorKind, WriteFailure};
    matches!(e.kind.as_ref(), ErrorKind::Write(WriteFailure::WriteError(w)) if w.code == 11000)
}
