pub(crate) mod admin;
mod health;
mod objects;

pub use admin::{list_buckets, register_bucket, reset_cache};
pub use health::{health, liveness, metrics, readiness};
pub use objects::get_object;
