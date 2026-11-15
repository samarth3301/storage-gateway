# S3 CDN Network

A caching edge service that serves objects from multiple S3-compatible buckets (Cloudflare R2, AWS S3, MinIO, Spaces) behind one URL scheme: `/{instance}/{object-key}`.

## Features

- **Multi-bucket**: buckets are registered at runtime and stored in MongoDB; their credentials are encrypted at rest with AES-256-GCM.
- **In-memory cache**: byte-bounded `moka` cache with TTL, plus a short-lived negative cache for 404s.
- **HTTP semantics**: `ETag` / `If-None-Match` (304), `Range` requests (206/416), `Cache-Control`, streaming responses.
- **Operations**: liveness/readiness probes, Prometheus metrics, JSON logs with request IDs, graceful shutdown.
- **Delivery**: CI (fmt, clippy, unit + integration tests, `cargo audit`), container image on GHCR, tag-driven deploys.

## Layout

```
src/
  main.rs        process entry: config, telemetry, serve, shutdown
  lib.rs         crate root
  app.rs         AppState, router and middleware
  config.rs      environment configuration
  error.rs       AppError and JSON error responses
  models.rs      domain types
  telemetry.rs   logging and metrics
  handlers/      HTTP endpoints (objects, admin, health)
  services/      MongoDB, S3 clients, caches, credential encryption
tests/           integration tests (MongoDB + S3)
docker/          Dockerfile and compose file
deploy/nginx/    TLS reverse proxy configuration
.github/         CI/CD workflow and Dependabot
```

## API

| Method | Path | Auth | Description |
|---|---|---|---|
| GET | `/{instance}/{key}` | – | Serve an object (supports `Range`, `If-None-Match`) |
| POST | `/buckets/register` | token | Register a bucket |
| GET | `/buckets` | token | List registered buckets (no secrets) |
| POST | `/reset-cache` | token | Clear object and 404 caches |
| GET | `/health` | – | Readiness with cache stats; 503 when MongoDB is down |
| GET | `/health/live` | – | Liveness probe |
| GET | `/health/ready` | – | Readiness probe |
| GET | `/metrics` | internal | Prometheus metrics (blocked at nginx) |

Token-protected endpoints expect the `X-API-Token` header.

```bash
curl -X POST https://cdn.example.com/buckets/register \
  -H "Content-Type: application/json" \
  -H "X-API-Token: $REGISTRATION_TOKEN" \
  -d '{
    "name": "assets",
    "endpoint": "https://<account>.r2.cloudflarestorage.com",
    "access_key": "...",
    "secret_key": "...",
    "bucket_name": "production-assets",
    "region": "auto",
    "force_path_style": false
  }'

curl https://cdn.example.com/assets/images/banner.jpg
```

Errors are JSON: `{"error": "not_found", "message": "...", "key": "...", "timestamp": 0}`.

## Configuration

| Variable | Default | Description |
|---|---|---|
| `REGISTRATION_TOKEN` | required | Admin API token |
| `CREDENTIALS_KEY` | required | Base64 32-byte key for credential encryption (`openssl rand -base64 32`) |
| `MONGODB_URI` | `mongodb://localhost:27017` | MongoDB connection string |
| `MONGODB_DATABASE` | `cdn_storage` | Database name |
| `SERVER_HOST` / `SERVER_PORT` | `0.0.0.0` / `9876` | Bind address |
| `CACHE_MAX_MB` | `512` | Total object cache size |
| `CACHE_MAX_OBJECT_MB` | `32` | Larger objects are streamed, not cached |
| `CACHE_TTL_SECONDS` | `3600` | Object cache TTL |
| `CACHE_NOT_FOUND_TTL_SECONDS` | `60` | 404 cache TTL |
| `RUST_LOG` | `s3_cdn_network=info` | Log filter |
| `LOG_FORMAT` | `default` | `json` for structured logs |

Existing plaintext credentials are encrypted automatically on startup. Keep `CREDENTIALS_KEY` in a secret store; losing it means re-registering buckets.

## Development

```bash
make setup          # create .env from .env.example
cargo run
make lint test
```

Integration tests run when both variables are set:

```bash
docker run -d -p 27017:27017 mongo:7
docker run -d -p 9090:9090 adobe/s3mock:4.7.0
INTEGRATION_MONGODB_URI=mongodb://localhost:27017 \
INTEGRATION_S3_ENDPOINT=http://localhost:9090 cargo test
```

## Deployment

1. Pushes to `main` publish `ghcr.io/samarth3301/cdn:main`; tags `vX.Y.Z` publish `X.Y.Z` and deploy.
2. Configure the `production` environment secrets: `DEPLOY_HOST`, `DEPLOY_USER`, `DEPLOY_SSH_KEY`, `DEPLOY_PATH` (repo checkout on the server).
3. On the server: `.env` from `.env.example`, `docker login ghcr.io`, then `docker compose -f docker/compose.yaml up -d`.
4. Install `deploy/nginx/cdn.conf` into nginx's `sites-enabled` and reload.

For global edge caching, put the hostname behind a CDN such as Cloudflare; this service then acts as the origin shield.

## Observability

Key metrics: `cdn_cache_requests_total{result}`, `cdn_origin_errors_total{kind}`, `http_request_duration_seconds{method,status}`. Every response carries `X-Request-Id`, which also appears in logs.

## License

MIT
