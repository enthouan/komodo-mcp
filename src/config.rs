use std::{env, fmt};

use reqwest::{Url, header::HeaderValue};

use crate::{capabilities::CapabilityPolicy, diagnostics::Redactor, error::AppError};

mod http;
pub use http::HttpConfig;
mod selection;
pub(crate) use selection::ToolSelection;

#[derive(Clone, Debug, Default)]
pub enum Transport {
    #[default]
    Stdio,
    Http(HttpConfig),
}

/// Validated once at startup. Values are deliberately absent from Debug.
#[derive(Clone)]
pub struct Config {
    pub(crate) address: String,
    pub(crate) key: String,
    pub(crate) secret: String,
    redactor: Redactor,
    capabilities: CapabilityPolicy,
    pub(crate) tools: ToolSelection,
    transport: Transport,
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
        let optional = |field| match env::var(field) {
            Ok(value) => Ok(Some(value)),
            Err(env::VarError::NotPresent) => Ok(None),
            Err(env::VarError::NotUnicode(_)) => {
                Err(AppError::configuration(field, "must contain valid Unicode"))
            }
        };
        let capabilities = CapabilityPolicy::parse(
            optional("KOMODO_ALLOW_OPERATE")?.as_deref(),
            optional("KOMODO_ALLOW_CONFIGURATION_EDIT")?.as_deref(),
        )?
        .with_pruning(crate::capabilities::parse_grant(
            "KOMODO_ALLOW_PRUNE",
            optional("KOMODO_ALLOW_PRUNE")?.as_deref(),
        )?)
        .with_terminal(crate::capabilities::parse_grant(
            "KOMODO_ALLOW_TERMINAL",
            optional("KOMODO_ALLOW_TERMINAL")?.as_deref(),
        )?);
        let mut config = Self::parse(
            &read("KOMODO_ADDRESS")?,
            &read("KOMODO_API_KEY")?,
            &read("KOMODO_API_SECRET")?,
        )?
        .with_capabilities(capabilities)
        .with_tool_selection(
            optional("KOMODO_ENABLED_TOOLS")?.as_deref(),
            optional("KOMODO_DISABLED_TOOLS")?.as_deref(),
        )?;
        match optional("KOMODO_TRANSPORT")?.as_deref() {
            None | Some("stdio") => {}
            Some("http") => {
                let tokens = read("KOMODO_HTTP_TOKENS")?;
                let http = HttpConfig::parse(
                    optional("KOMODO_HTTP_BIND")?.as_deref(),
                    &tokens,
                    optional("KOMODO_HTTP_ALLOWED_ORIGINS")?.as_deref(),
                )?;
                if http.authenticate(&config.key).is_some()
                    || http.authenticate(&config.secret).is_some()
                {
                    return Err(AppError::configuration(
                        "KOMODO_HTTP_TOKENS",
                        "use inbound tokens distinct from the Core credentials",
                    ));
                }
                config.redactor = Redactor::new(
                    [config.key.as_str(), config.secret.as_str()]
                        .into_iter()
                        .chain(tokens.split(',').map(str::trim))
                        .map(str::to_owned),
                );
                config.transport = Transport::Http(http);
            }
            Some(_) => {
                return Err(AppError::configuration(
                    "KOMODO_TRANSPORT",
                    "use stdio or http",
                ));
            }
        }
        Ok(config)
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
        let redactor = Redactor::new([key.to_owned(), secret.to_owned()]);
        Ok(Self {
            // The SDK concatenates the address and `/read`; retain the path prefix.
            address: url.as_str().trim_end_matches('/').to_owned(),
            key: key.to_owned(),
            secret: secret.to_owned(),
            redactor,
            capabilities: CapabilityPolicy::default(),
            tools: ToolSelection::default(),
            transport: Transport::Stdio,
        })
    }

    pub fn with_capabilities(mut self, capabilities: CapabilityPolicy) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn capabilities(&self) -> CapabilityPolicy {
        self.capabilities
    }

    /// Select from the full registered catalog. Capability and Core checks still apply.
    pub fn with_tool_selection(
        mut self,
        enabled: Option<&str>,
        disabled: Option<&str>,
    ) -> Result<Self, AppError> {
        self.tools = ToolSelection::parse(enabled, disabled)?;
        Ok(self)
    }

    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    pub(crate) fn redactor(&self) -> Redactor {
        self.redactor.clone()
    }
}

impl fmt::Debug for Config {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Config { KOMODO_ADDRESS: [redacted], KOMODO_API_KEY: [redacted], KOMODO_API_SECRET: [redacted] }")
    }
}
