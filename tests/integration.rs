use aws_sdk_s3::config::{BehaviorVersion, Credentials, Region};
use aws_sdk_s3::primitives::ByteStream;
use axum::body::{to_bytes, Body};
use axum::http::{header, Request, StatusCode};
use axum::Router;
use mongodb::bson::{doc, Document};
use s3_cdn_network::app::{create_app, AppState};
use s3_cdn_network::config::{AuthConfig, CacheConfig, Config, DatabaseConfig, ServerConfig};
use std::time::Duration;
use tower::ServiceExt;

const TOKEN: &str = "integration-token";
const KEY: &str = "MDEyMzQ1Njc4OWFiY2RlZjAxMjM0NTY3ODlhYmNkZWY=";
const ACCESS_KEY: &str = "minioadmin";
const SECRET_KEY: &str = "minioadmin";
const BUCKET: &str = "cdn-integration";
const BODY: &[u8] = b"hello world";

struct Env {
    mongo_uri: String,
    s3_endpoint: String,
}

fn env() -> Option<Env> {
    Some(Env {
        mongo_uri: std::env::var("INTEGRATION_MONGODB_URI").ok()?,
        s3_endpoint: std::env::var("INTEGRATION_S3_ENDPOINT").ok()?,
    })
}

async fn seed_bucket(endpoint: &str) {
    let config = aws_sdk_s3::Config::builder()
        .behavior_version(BehaviorVersion::latest())
        .region(Region::new("us-east-1"))
        .endpoint_url(endpoint)
        .force_path_style(true)
        .credentials_provider(Credentials::new(ACCESS_KEY, SECRET_KEY, None, None, "test"))
        .build();
    let s3 = aws_sdk_s3::Client::from_conf(config);
    let _ = s3.create_bucket().bucket(BUCKET).send().await;
    s3.put_object()
        .bucket(BUCKET)
        .key("docs/hello.txt")
        .content_type("text/plain")
        .body(ByteStream::from_static(BODY))
        .send()
        .await
        .expect("seed object uploads to MinIO");
}

async fn send(
    app: &Router,
    request: Request<Body>,
) -> (StatusCode, axum::http::HeaderMap, Vec<u8>) {
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
    (status, headers, body.to_vec())
}

fn get(uri: &str) -> Request<Body> {
    Request::get(uri).body(Body::empty()).unwrap()
}

#[tokio::test]
async fn serves_caches_and_protects_objects() {
    let Some(env) = env() else {
        eprintln!("skipping: set INTEGRATION_MONGODB_URI and INTEGRATION_S3_ENDPOINT");
        return;
    };
    seed_bucket(&env.s3_endpoint).await;

    let database = format!("cdn_it_{}", std::process::id());
    let config = Config {
        server: ServerConfig {
            host: "127.0.0.1".into(),
            port: 0,
        },
        database: DatabaseConfig {
            uri: env.mongo_uri.clone(),
            name: database.clone(),
        },
        auth: AuthConfig {
            registration_token: TOKEN.into(),
            credentials_key: KEY.into(),
        },
        cache: CacheConfig {
            max_bytes: 16 * 1024 * 1024,
            max_object_bytes: 1024 * 1024,
            time_to_live_seconds: 60,
            not_found_ttl_seconds: 60,
        },
    };
    let state = AppState::init(&config)
        .await
        .expect("app state initialises");
    let db = state.db.clone();
    let cache = state.cache.clone();
    let app = create_app(state);

    let registration = serde_json::json!({
        "name": "it",
        "endpoint": env.s3_endpoint,
        "access_key": ACCESS_KEY,
        "secret_key": SECRET_KEY,
        "bucket_name": BUCKET,
        "region": "us-east-1",
        "force_path_style": true
    })
    .to_string();
    let register = |token: &str| {
        Request::post("/buckets/register")
            .header(header::CONTENT_TYPE, "application/json")
            .header("x-api-token", token)
            .body(Body::from(registration.clone()))
            .unwrap()
    };

    assert_eq!(
        send(&app, register("wrong")).await.0,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(send(&app, register(TOKEN)).await.0, StatusCode::CREATED);
    assert_eq!(send(&app, register(TOKEN)).await.0, StatusCode::CONFLICT);

    let stored: Document = db
        .collection("buckets")
        .find_one(doc! {"name": "it"}, None)
        .await
        .unwrap()
        .unwrap();
    assert!(stored.get_str("secret_key").unwrap().starts_with("v1:"));

    let (status, headers, body) = send(&app, get("/it/docs/hello.txt")).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, BODY);
    let etag = headers
        .get(header::ETAG)
        .expect("etag is forwarded")
        .clone();

    for _ in 0..100 {
        if cache.contains_key("it:docs/hello.txt") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(cache.contains_key("it:docs/hello.txt"));

    let conditional = Request::get("/it/docs/hello.txt")
        .header(header::IF_NONE_MATCH, etag)
        .body(Body::empty())
        .unwrap();
    assert_eq!(send(&app, conditional).await.0, StatusCode::NOT_MODIFIED);

    let ranged = Request::get("/it/docs/hello.txt")
        .header(header::RANGE, "bytes=0-4")
        .body(Body::empty())
        .unwrap();
    let (status, headers, body) = send(&app, ranged).await;
    assert_eq!(status, StatusCode::PARTIAL_CONTENT);
    assert_eq!(body, b"hello");
    assert_eq!(headers[header::CONTENT_RANGE], "bytes 0-4/11");

    assert_eq!(
        send(&app, get("/it/docs/missing.txt")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        send(&app, get("/nope/docs/hello.txt")).await.0,
        StatusCode::NOT_FOUND
    );
    assert_eq!(send(&app, get("/health/ready")).await.0, StatusCode::OK);
    assert_eq!(
        send(&app, get("/buckets")).await.0,
        StatusCode::UNAUTHORIZED
    );

    let reset = Request::post("/reset-cache").body(Body::empty()).unwrap();
    assert_eq!(send(&app, reset).await.0, StatusCode::UNAUTHORIZED);

    db.drop(None).await.unwrap();
}
