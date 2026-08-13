use std::{collections::HashMap, net::SocketAddr};

use thiserror::Error;
use url::Url;

const DEFAULT_BIND_ADDRESS: &str = "127.0.0.1:8080";

#[derive(Clone, Debug)]
pub struct Config {
    pub bind_address: SocketAddr,
    pub supabase_url: Url,
    pub database_url: Option<String>,
}

#[derive(Debug, Error, PartialEq)]
pub enum ConfigError {
    #[error("required environment variable {0} is missing")]
    Missing(&'static str),
    #[error("BIND_ADDRESS must be a valid socket address")]
    InvalidBindAddress,
    #[error("SUPABASE_URL must be a valid URL")]
    InvalidSupabaseUrl,
    #[error("SUPABASE_URL must use HTTPS outside localhost")]
    InsecureSupabaseUrl,
    #[error("SUPABASE_URL must be a base URL without credentials, path, query, or fragment")]
    InvalidSupabaseBaseUrl,
    #[error("DATABASE_URL must use the postgres or postgresql scheme")]
    InvalidDatabaseUrl,
}

impl Config {
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_values(std::env::vars().collect())
    }

    fn from_values(values: HashMap<String, String>) -> Result<Self, ConfigError> {
        let bind_address = values
            .get("BIND_ADDRESS")
            .map(String::as_str)
            .unwrap_or(DEFAULT_BIND_ADDRESS)
            .parse()
            .map_err(|_| ConfigError::InvalidBindAddress)?;
        let supabase_url = required(&values, "SUPABASE_URL")?
            .parse::<Url>()
            .map_err(|_| ConfigError::InvalidSupabaseUrl)?;
        validate_supabase_url(&supabase_url)?;
        let database_url = values
            .get("DATABASE_URL")
            .map(|value| value.trim().to_owned())
            .filter(|value| !value.is_empty());
        if database_url.as_deref().is_some_and(|value| {
            !value.starts_with("postgres://") && !value.starts_with("postgresql://")
        }) {
            return Err(ConfigError::InvalidDatabaseUrl);
        }

        Ok(Self {
            bind_address,
            supabase_url,
            database_url,
        })
    }
}

fn required<'a>(
    values: &'a HashMap<String, String>,
    name: &'static str,
) -> Result<&'a str, ConfigError> {
    values
        .get(name)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or(ConfigError::Missing(name))
}

fn validate_supabase_url(url: &Url) -> Result<(), ConfigError> {
    let is_local = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "::1"));
    let secure_transport = url.scheme() == "https" || (url.scheme() == "http" && is_local);
    let clean_base = url.username().is_empty()
        && url.password().is_none()
        && url.query().is_none()
        && url.fragment().is_none()
        && matches!(url.path(), "" | "/");
    if !secure_transport {
        return Err(ConfigError::InsecureSupabaseUrl);
    }
    if !clean_base {
        return Err(ConfigError::InvalidSupabaseBaseUrl);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Config, ConfigError};
    use std::collections::HashMap;

    fn values(url: &str) -> HashMap<String, String> {
        HashMap::from([("SUPABASE_URL".to_owned(), url.to_owned())])
    }

    #[test]
    fn loads_secure_configuration_with_default_bind_address() {
        let config = Config::from_values(values("https://example.supabase.co")).unwrap();
        assert_eq!(config.bind_address.to_string(), "127.0.0.1:8080");
    }

    #[test]
    fn permits_http_only_for_local_supabase() {
        assert!(Config::from_values(values("http://127.0.0.1:54321")).is_ok());
        assert_eq!(
            Config::from_values(values("http://example.supabase.co")).unwrap_err(),
            ConfigError::InsecureSupabaseUrl
        );
    }

    #[test]
    fn rejects_non_base_supabase_url() {
        assert_eq!(
            Config::from_values(values("https://example.supabase.co/other")).unwrap_err(),
            ConfigError::InvalidSupabaseBaseUrl
        );
    }

    #[test]
    fn accepts_an_optional_postgres_database_url() {
        let mut values = values("https://example.supabase.co");
        values.insert(
            "DATABASE_URL".to_owned(),
            "postgresql://postgres:secret@db.example.test/postgres".to_owned(),
        );
        assert!(Config::from_values(values).unwrap().database_url.is_some());
    }

    #[test]
    fn rejects_an_invalid_database_url_scheme() {
        let mut values = values("https://example.supabase.co");
        values.insert("DATABASE_URL".to_owned(), "https://example.test".to_owned());
        assert_eq!(
            Config::from_values(values).unwrap_err(),
            ConfigError::InvalidDatabaseUrl
        );
    }
}
