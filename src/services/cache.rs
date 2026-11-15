use moka::future::Cache;
use std::time::Duration;
use tracing::info;

use crate::config::CacheConfig;
use crate::models::CachedObject;

const MAX_NOT_FOUND_ENTRIES: u64 = 100_000;

pub fn object_cache(config: &CacheConfig) -> Cache<String, CachedObject> {
    info!(
        max_bytes = config.max_bytes,
        max_object_bytes = config.max_object_bytes,
        ttl_seconds = config.time_to_live_seconds,
        "Cache configured"
    );

    Cache::builder()
        .weigher(|key: &String, value: &CachedObject| {
            u32::try_from(key.len() + value.data.len()).unwrap_or(u32::MAX)
        })
        .max_capacity(config.max_bytes)
        .time_to_live(Duration::from_secs(config.time_to_live_seconds))
        .build()
}

pub fn not_found_cache(config: &CacheConfig) -> Cache<String, ()> {
    Cache::builder()
        .max_capacity(MAX_NOT_FOUND_ENTRIES)
        .time_to_live(Duration::from_secs(config.not_found_ttl_seconds))
        .build()
}
