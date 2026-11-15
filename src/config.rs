use std::env;
use std::str::FromStr;

#[derive(Debug, Clone)]
pub struct Config {
    pub server: ServerConfig,
    pub database: DatabaseConfig,
    pub auth: AuthConfig,
    pub cache: CacheConfig,
}

#[derive(Debug, Clone)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
}

#[derive(Clone)]
pub struct AuthConfig {
    pub registration_token: String,
    pub credentials_key: String,
}

impl std::fmt::Debug for AuthConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthConfig { .. }")
    }
}

#[derive(Debug, Clone)]
pub struct DatabaseConfig {
    pub uri: String,
    pub name: String,
}

#[derive(Debug, Clone)]
pub struct CacheConfig {
    pub max_bytes: u64,
    pub max_object_bytes: u64,
    pub time_to_live_seconds: u64,
    pub not_found_ttl_seconds: u64,
}

const MIB: u64 = 1024 * 1024;

fn var_or(name: &str, default: &str) -> String {
    env::var(name).unwrap_or_else(|_| default.to_string())
}

fn required(name: &'static str) -> Result<String, ConfigError> {
    env::var(name)
        .ok()
        .filter(|v| !v.trim().is_empty())
        .ok_or(ConfigError::Missing(name))
}

fn parse_var<T: FromStr>(name: &'static str, default: T) -> Result<T, ConfigError> {
    match env::var(name) {
        Ok(raw) => raw.trim().parse().map_err(|_| ConfigError::Invalid(name)),
        Err(_) => Ok(default),
    }
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            server: ServerConfig {
                host: var_or("SERVER_HOST", "0.0.0.0"),
                port: parse_var("SERVER_PORT", 9876)?,
            },
            database: DatabaseConfig {
                uri: var_or("MONGODB_URI", "mongodb://localhost:27017"),
                name: var_or("MONGODB_DATABASE", "cdn_storage"),
            },
            auth: AuthConfig {
                registration_token: required("REGISTRATION_TOKEN")?,
                credentials_key: required("CREDENTIALS_KEY")?,
            },
            cache: CacheConfig {
                max_bytes: parse_var("CACHE_MAX_MB", 512u64)? * MIB,
                max_object_bytes: parse_var("CACHE_MAX_OBJECT_MB", 32u64)? * MIB,
                time_to_live_seconds: parse_var("CACHE_TTL_SECONDS", 3600)?,
                not_found_ttl_seconds: parse_var("CACHE_NOT_FOUND_TTL_SECONDS", 60)?,
            },
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Missing required environment variable: {0}")]
    Missing(&'static str),
    #[error("Invalid value for environment variable: {0}")]
    Invalid(&'static str),
}
