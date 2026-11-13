use aws_sdk_s3::Client;
use axum::http::HeaderValue;
use bytes::Bytes;
use mongodb::bson::oid::ObjectId;
use serde::{Deserialize, Serialize};

#[derive(Clone)]
pub struct CachedObject {
    pub data: Bytes,
    pub content_type: HeaderValue,
    pub etag: Option<HeaderValue>,
}

#[derive(Clone)]
pub struct BucketClient {
    pub client: Client,
    pub bucket: String,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct BucketCredentials {
    #[serde(rename = "_id", skip_serializing_if = "Option::is_none")]
    pub id: Option<ObjectId>,
    pub name: String,
    pub endpoint: String,
    pub access_key: String,
    pub secret_key: String,
    pub bucket_name: String,
    pub region: String,
    #[serde(default)]
    pub force_path_style: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct BucketStatus {
    pub name: String,
    pub bucket_name: String,
    pub endpoint: String,
    pub region: String,
}
