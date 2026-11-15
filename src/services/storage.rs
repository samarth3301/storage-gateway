use aws_sdk_s3::config::timeout::TimeoutConfig;
use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::Client;
use futures::StreamExt;
use moka::future::Cache;
use mongodb::bson::doc;
use mongodb::Database;
use std::time::Duration;
use tracing::{error, info, warn};

use crate::models::{BucketClient, BucketCredentials};
use crate::services::crypto::{Cipher, CryptoError};
use crate::services::db::buckets;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const READ_TIMEOUT: Duration = Duration::from_secs(30);

pub fn create_s3_client(
    credentials: &BucketCredentials,
    cipher: &Cipher,
) -> Result<BucketClient, CryptoError> {
    let access_key = cipher.decrypt(&credentials.access_key)?;
    let secret_key = cipher.decrypt(&credentials.secret_key)?;

    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new(credentials.region.clone()))
        .endpoint_url(&credentials.endpoint)
        .force_path_style(credentials.force_path_style)
        .credentials_provider(Credentials::new(
            access_key,
            secret_key,
            None,
            None,
            "registered-bucket",
        ))
        .timeout_config(
            TimeoutConfig::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .build(),
        )
        .build();

    Ok(BucketClient {
        client: Client::from_conf(config),
        bucket: credentials.bucket_name.clone(),
    })
}

pub fn seal(credentials: &mut BucketCredentials, cipher: &Cipher) {
    if !Cipher::is_encrypted(&credentials.access_key) {
        credentials.access_key = cipher.encrypt(&credentials.access_key);
    }
    if !Cipher::is_encrypted(&credentials.secret_key) {
        credentials.secret_key = cipher.encrypt(&credentials.secret_key);
    }
}

async fn encrypt_legacy_entry(db: &Database, creds: &BucketCredentials, cipher: &Cipher) {
    let (Some(id), false) = (creds.id, Cipher::is_encrypted(&creds.secret_key)) else {
        return;
    };
    let mut sealed = creds.clone();
    seal(&mut sealed, cipher);
    let update =
        doc! {"$set": {"access_key": &sealed.access_key, "secret_key": &sealed.secret_key}};
    match buckets(db).update_one(doc! {"_id": id}, update, None).await {
        Ok(_) => info!(name = %creds.name, "Encrypted legacy plaintext credentials"),
        Err(e) => warn!(name = %creds.name, "Failed to encrypt legacy credentials: {}", e),
    }
}

pub async fn preload_clients(
    db: &Database,
    clients: &Cache<String, BucketClient>,
    cipher: &Cipher,
) -> Result<(), mongodb::error::Error> {
    let mut cursor = buckets(db).find(None, None).await?;
    let mut count = 0;
    while let Some(result) = cursor.next().await {
        let creds = match result {
            Ok(c) => c,
            Err(e) => {
                error!("Error reading bucket from database: {}", e);
                continue;
            }
        };
        encrypt_legacy_entry(db, &creds, cipher).await;
        match create_s3_client(&creds, cipher) {
            Ok(client) => {
                clients.insert(creds.name.clone(), client).await;
                count += 1;
            }
            Err(e) => error!(name = %creds.name, "Cannot load bucket credentials: {}", e),
        }
    }
    info!("Preloaded {} bucket client(s)", count);
    Ok(())
}
