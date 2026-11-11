use std::{env, fmt};

use reqwest::{Url, header::HeaderValue};

use crate::{diagnostics::Redactor, error::AppError};

/// Validated once at startup. Values are deliberately absent from Debug.
#[derive(Clone)]
pub struct Config {
    pub(crate) address: String,
    pub(crate) key: String,
    pub(crate) secret: String,
}

impl Config {
    pub fn from_env() -> Result<Self, AppError> {
        let read = |field| {
            env::var(field).map_err(|error| {
                AppError::configuration(
                    field,
                    match error {
                        env::VarError::NotPresent => "is required; set this environment variable",
                        env::VarError::NotUnicode(_) => "must contain valid Unicode",
                    },
                )
            })
        };
        Self::parse(
            &read("KOMODO_ADDRESS")?,
            &read("KOMODO_API_KEY")?,
            &read("KOMODO_API_SECRET")?,
        )
    }

    /// Takes values directly so tests never mutate the process environment.
    pub fn parse(address: &str, key: &str, secret: &str) -> Result<Self, AppError> {
        let invalid_address = |reason| AppError::configuration("KOMODO_ADDRESS", reason);
        if !address.contains("://")
            || address.contains('\\')
            || address
                .chars()
                .any(|character| character.is_control() || character.is_whitespace())
        {
            return Err(invalid_address(
                "must be an absolute HTTP or HTTPS URL without whitespace",
            ));
        }
        let url = Url::parse(address)
            .map_err(|_| invalid_address("must be an absolute HTTP or HTTPS URL"))?;
        if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
            return Err(invalid_address("must be an absolute HTTP or HTTPS URL"));
        }
        // Check the original authority too: URL normalization discards empty userinfo.
        let authority = address
            .split_once("://")
            .map(|(_, rest)| rest.split(['/', '?', '#']).next().unwrap_or_default());
        if authority.is_none_or(str::is_empty) {
            return Err(invalid_address(
                "must include a host after HTTP:// or HTTPS://",
            ));
        }
        if !url.username().is_empty()
            || url.password().is_some()
            || authority.is_some_and(|value| value.contains('@'))
        {
            return Err(invalid_address(
                "must not contain user information; use the credential variables",
            ));
        }
        if url.query().is_some() || url.fragment().is_some() {
            return Err(invalid_address(
                "must not contain a query string or fragment",
            ));
        }
        for (field, value) in [("KOMODO_API_KEY", key), ("KOMODO_API_SECRET", secret)] {
            if value.trim().is_empty() {
                return Err(AppError::configuration(field, "must not be empty"));
            }
            HeaderValue::from_str(value).map_err(|_| {
                AppError::configuration(field, "must contain only valid HTTP header characters")
            })?;
        }
        Ok(Self {
            // The SDK concatenates the address and `/read`; retain the path prefix.
            address: url.as_str().trim_end_matches('/').to_owned(),
            key: key.to_owned(),
            secret: secret.to_owned(),
        })
    }

    pub(crate) fn redactor(&self) -> Redactor {
        Redactor::new(self.key.clone(), self.secret.clone())
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Config { KOMODO_ADDRESS: [redacted], KOMODO_API_KEY: [redacted], KOMODO_API_SECRET: [redacted] }")
    }
}
