use std::collections::BTreeSet;

use rmcp::handler::server::tool::ToolRouter;

use crate::{error::AppError, server::KomodoServer};

#[derive(Clone, Debug, Default)]
pub(crate) struct ToolSelection {
    enabled: Option<BTreeSet<String>>,
    disabled: BTreeSet<String>,
}

impl ToolSelection {
    pub(super) fn parse(enabled: Option<&str>, disabled: Option<&str>) -> Result<Self, AppError> {
        if enabled.is_none() && disabled.is_none() {
            return Ok(Self::default());
        }
        let registry = KomodoServer::tool_router();
        Ok(Self {
            enabled: enabled
                .map(|raw| names("KOMODO_ENABLED_TOOLS", raw, &registry))
                .transpose()?,
            disabled: disabled
                .map(|raw| names("KOMODO_DISABLED_TOOLS", raw, &registry))
                .transpose()?
                .unwrap_or_default(),
        })
    }

    pub(crate) fn allows(&self, name: &str) -> bool {
        self.enabled
            .as_ref()
            .is_none_or(|names| names.contains(name))
            && !self.disabled.contains(name)
    }
}

fn names(
    field: &'static str,
    raw: &str,
    registry: &ToolRouter<KomodoServer>,
) -> Result<BTreeSet<String>, AppError> {
    let mut tools = BTreeSet::new();
    for (offset, name) in raw.split(',').map(str::trim).enumerate() {
        let invalid = |correction| AppError {
            configuration_entry: Some(offset + 1),
            ..AppError::configuration(field, correction)
        };
        if name.is_empty() {
            return Err(invalid(
                "is blank. Supply an exact tool name or unset the variable",
            ));
        }
        if !registry.has_route(name) {
            return Err(invalid(
                "is an unknown tool. Use an exact name from the full tool catalog",
            ));
        }
        tools.insert(name.to_owned());
    }
    Ok(tools)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unset_and_partial_settings_intersect_and_subtract_exact_names() {
        let defaults = ToolSelection::parse(None, None).unwrap();
        let deny = ToolSelection::parse(None, Some(" get_image_history ")).unwrap();
        for tool in KomodoServer::tool_router().list_all() {
            assert!(defaults.allows(&tool.name));
            assert_eq!(deny.allows(&tool.name), tool.name != "get_image_history");
        }
        let triage = ToolSelection::parse(
            Some(" list_stacks, get_stack ,stack_diagnose, get_stack "),
            None,
        )
        .unwrap();
        let names: Vec<_> = KomodoServer::tool_router()
            .list_all()
            .into_iter()
            .filter(|tool| triage.allows(&tool.name))
            .map(|tool| tool.name)
            .collect();
        assert_eq!(names, ["get_stack", "list_stacks", "stack_diagnose"]);

        let none = ToolSelection::parse(Some("get_stack"), Some("get_stack")).unwrap();
        assert!(
            KomodoServer::tool_router()
                .list_all()
                .iter()
                .all(|tool| !none.allows(&tool.name))
        );
        // Existence is independent of local grants. The capability router decides availability.
        assert!(
            ToolSelection::parse(Some("execute_terminal"), None)
                .unwrap()
                .allows("execute_terminal")
        );
    }

    #[test]
    fn invalid_entries_fail_closed_with_the_setting_and_one_based_position() {
        for field in ["KOMODO_ENABLED_TOOLS", "KOMODO_DISABLED_TOOLS"] {
            for (raw, position, cause) in [
                ("", 1, "is blank"),
                ("   ", 1, "is blank"),
                (",list_stacks", 1, "is blank"),
                ("list_stacks, ,get_stack", 2, "is blank"),
                ("list_stacks,", 2, "is blank"),
                ("get_stack,NOT_AN_EXISTING_TOOL", 2, "unknown tool"),
                ("list_*", 1, "unknown tool"),
            ] {
                let (enabled, disabled) = if field == "KOMODO_ENABLED_TOOLS" {
                    (Some(raw), None)
                } else {
                    (Some("list_stacks"), Some(raw))
                };
                let error = ToolSelection::parse(enabled, disabled).unwrap_err();
                assert_eq!(error.field, Some(field));
                assert_eq!(error.configuration_entry, Some(position));
                let diagnostic = error.to_string();
                assert!(diagnostic.contains(&format!("{field} entry {position}")));
                assert!(diagnostic.contains(cause));
                assert!(!diagnostic.contains("NOT_AN_EXISTING_TOOL"));
                assert!(
                    serde_json::to_value(&error)
                        .unwrap()
                        .get("configuration_entry")
                        .is_none()
                );
            }
        }
    }
}
