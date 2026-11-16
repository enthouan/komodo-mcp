//! Local grants restrict exposure; Core remains the resource permission authority.

use rmcp::handler::server::tool::ToolRouter;

use crate::error::AppError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    Read,
    Operate,
    ConfigurationEdit,
    /// Deliberate deletion can implicitly remove live workloads through Core.
    /// Both independent startup grants are necessary.
    Destructive,
    /// Docker category pruning requires its own opt-in and operate, not edit.
    Prune,
    /// One-command terminals require their separate opt-in and operate.
    Terminal,
}

/// Independent, immutable startup grants. Reads are always locally enabled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CapabilityPolicy {
    operate: bool,
    configuration_edit: bool,
    pruning: bool,
    terminal: bool,
}

pub(crate) fn parse_grant(field: &'static str, value: Option<&str>) -> Result<bool, AppError> {
    match value {
        None | Some("false") => Ok(false),
        Some("true") => Ok(true),
        _ => Err(AppError::configuration(
            field,
            "must be exactly true or false, or unset for false",
        )),
    }
}

impl CapabilityPolicy {
    pub fn new(operate: bool, configuration_edit: bool) -> Self {
        Self {
            operate,
            configuration_edit,
            pruning: false,
            terminal: false,
        }
    }

    /// Missing values preserve read-only defaults. Typos never enable a grant.
    pub fn parse(
        operate: Option<&str>,
        configuration_edit: Option<&str>,
    ) -> Result<Self, AppError> {
        Ok(Self::new(
            parse_grant("KOMODO_ALLOW_OPERATE", operate)?,
            parse_grant("KOMODO_ALLOW_CONFIGURATION_EDIT", configuration_edit)?,
        ))
    }

    pub fn with_pruning(mut self, enabled: bool) -> Self {
        self.pruning = enabled;
        self
    }

    pub fn pruning_enabled(self) -> bool {
        self.pruning
    }

    pub fn with_terminal(mut self, enabled: bool) -> Self {
        self.terminal = enabled;
        self
    }

    pub fn terminal_enabled(self) -> bool {
        self.terminal
    }

    pub fn allows(self, capability: Capability) -> bool {
        match capability {
            Capability::Read => true,
            Capability::Operate => self.operate,
            Capability::ConfigurationEdit => self.configuration_edit,
            Capability::Destructive => self.operate && self.configuration_edit,
            Capability::Prune => self.operate && self.pruning,
            Capability::Terminal => self.operate && self.terminal,
        }
    }
}

/// The SDK uses this same restricted router for discovery, lookup and dispatch.
/// Unclassified routes fail closed, even with both mutation grants enabled.
pub fn restrict_router<S: Send + Sync + 'static>(
    mut router: ToolRouter<S>,
    policy: CapabilityPolicy,
    capability: impl Fn(&str) -> Option<Capability>,
) -> ToolRouter<S> {
    for tool in router.list_all() {
        if !capability(&tool.name).is_some_and(|required| policy.allows(required)) {
            router.disable_route(tool.name);
        }
    }
    router
}
