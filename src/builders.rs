//! Explicit existing-Server builders. Never inherit either SDK enum default.

use std::{collections::HashSet, time::Instant};

use komodo_client::{
    api::{
        read::GetBuilder,
        write::{CreateBuilder, UpdateBuilder},
    },
    entities::{
        builder::{
            Builder, BuilderConfig, BuilderListItem, PartialBuilderConfig,
            PartialServerBuilderConfig, ServerBuilderConfig,
        },
        to_general_name,
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    capabilities::Capability,
    client::CoreClient,
    configuration::{
        self as common, ConfigurationOutcome, ConfigurationResult, ConfigurationTarget, Preview,
        present,
    },
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput, ListInput, Page},
};

const OP: &str = "builder_configuration";
pub const SUPPORT: &str = "Direct Builder writes and Build execution support existing-Server builders only. AWS configuration/provisioning is deferred; URL builders are unsupported. Select an explicit Server builder; this tool never converts an existing AWS/URL builder.";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ServerKind {
    Server,
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerPatch {
    /// Must explicitly be server; no SDK-default, AWS or URL configuration.
    pub kind: ServerKind,
    /// Ordered fallback priority. Omit to preserve; [] deliberately unassigns all Servers.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<DetailInput>")]
    pub servers: Option<Vec<DetailInput>>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub target: DetailInput,
    pub patch: ServerPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: DetailInput,
    pub patch: ServerPatch,
    pub preview_id: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub name: String,
    #[schemars(schema_with = "create_schema")]
    pub config: ServerPatch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApplyInput {
    pub name: String,
    #[schemars(schema_with = "create_schema")]
    pub config: ServerPatch,
    pub preview_id: String,
}
fn create_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"allOf":[g.subschema_for::<ServerPatch>()],"required":["servers"],"properties":{"servers":{"minItems":1,"maxItems":16}}})
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct BuilderSummary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub builder_type: &'static str,
    pub supported: bool,
    pub support_limit: &'static str,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub builder_type: &'static str,
    pub supported: bool,
    pub ordered_server_ids: Vec<String>,
    pub server_count: usize,
    pub server_ids_complete: bool,
    pub support_limit: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct BuilderDetail {
    #[serde(flatten)]
    pub summary: BuilderSummary,
    pub definition: DefinitionSummary,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub requires_operate_to_apply: bool,
    pub reference_validation: &'static str,
}

pub(crate) fn unsupported() -> AppError {
    AppError {
        correction: Some(SUPPORT),
        ..AppError::new(ErrorKind::UnsupportedTarget, OP, None)
    }
}
pub(crate) fn require_operate(
    client: &CoreClient,
    required: bool,
    op: &'static str,
) -> Result<(), AppError> {
    if required && !client.capabilities().allows(Capability::Operate) {
        return Err(AppError {
            field: Some("KOMODO_ALLOW_OPERATE"),
            correction: Some(
                "This definition change can affect automatic execution or perform Core-side Git/Periphery cache work. Both configuration-edit and operate grants are required. No Core request was submitted.",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, op, None)
        });
    }
    Ok(())
}
pub(crate) fn server_config(b: &Builder) -> Result<&ServerBuilderConfig, AppError> {
    match &b.config {
        BuilderConfig::Server(c) if !b.template => Ok(c),
        _ => Err(unsupported()),
    }
}
pub(crate) fn summary(c: &BuilderConfig) -> DefinitionSummary {
    let (kind, ids) = match c {
        BuilderConfig::Server(c) => ("server", Some(&c.server_ids)),
        BuilderConfig::Aws(_) => ("aws", None),
        BuilderConfig::Url(_) => ("url", None),
    };
    DefinitionSummary {
        builder_type: kind,
        supported: ids.is_some(),
        ordered_server_ids: ids
            .into_iter()
            .flatten()
            .filter(|id| crate::deletion::object_id(id))
            .take(16)
            .cloned()
            .collect(),
        server_count: ids.map_or(0, Vec::len),
        server_ids_complete: ids
            .is_some_and(|v| v.len() <= 16 && v.iter().all(|id| crate::deletion::object_id(id))),
        support_limit: SUPPORT,
    }
}
fn item(client: &CoreClient, r: BuilderListItem) -> BuilderSummary {
    let kind = match r.info.builder_type.as_str() {
        "Server" => "server",
        "Aws" => "aws",
        "Url" => "url",
        _ => "unknown",
    };
    BuilderSummary {
        id: r.id,
        name: common::bounded_text(client, &r.name, 256),
        template: r.template,
        builder_type: kind,
        supported: kind == "server" && !r.template,
        support_limit: SUPPORT,
    }
}
pub async fn list(client: &CoreClient, input: ListInput) -> Result<Page<BuilderSummary>, AppError> {
    let p = inventory::list_builder_records(&client.bounded_evidence(), input).await?;
    Ok(Page {
        items: p.items.into_iter().map(|r| item(client, r)).collect(),
        page: p.page,
        page_size: p.page_size,
        has_more: p.has_more,
        next_page: p.next_page,
    })
}
pub async fn get(client: &CoreClient, input: DetailInput) -> Result<BuilderDetail, AppError> {
    let (listed, b) = inventory::resolve_builder(&client.bounded_evidence(), input).await?;
    let mut projected = item(client, listed);
    let definition = summary(&b.config);
    projected.builder_type = definition.builder_type;
    projected.supported = definition.supported && !b.template;
    Ok(BuilderDetail {
        summary: projected,
        definition,
    })
}

async fn servers(
    client: &CoreClient,
    selectors: &[DetailInput],
) -> Result<(Vec<String>, Value), AppError> {
    if selectors.len() > 16 {
        return Err(AppError::arguments(
            OP,
            "Select at most 16 ordered existing Servers",
        ));
    }
    let mut ids = Vec::new();
    let mut refs = Vec::new();
    let mut seen = HashSet::new();
    for target in selectors {
        let (_, s) = inventory::resolve_server(client, target.clone()).await?;
        if s.template || !seen.insert(s.id.clone()) {
            return Err(unsupported());
        }
        ids.push(s.id.clone());
        refs.push(
            json!({"server":common::resource_baseline("server",&s),"public_key":s.info.public_key}),
        );
    }
    Ok((ids, json!(refs)))
}
/// Bind the actual Server variant and its ordered Servers/trust, including when
/// a Build preserves rather than replaces its Builder association.
pub(crate) async fn resolve_supported(
    client: &CoreClient,
    target: DetailInput,
) -> Result<(Builder, Value), AppError> {
    let (_, b) = inventory::resolve_builder(client, target).await?;
    let c = server_config(&b)?;
    let selectors = c
        .server_ids
        .iter()
        .map(|id| DetailInput {
            id: Some(id.clone()),
            name: None,
        })
        .collect::<Vec<_>>();
    let (_, refs) = servers(client, &selectors).await?;
    let evidence = json!({"builder":common::resource_baseline("builder",&b),"servers":refs});
    Ok((b, evidence))
}
async fn prepare(
    client: &CoreClient,
    p: &ServerPatch,
) -> Result<(PartialBuilderConfig, Value), AppError> {
    common::validate_payload(client, p)?;
    let (server_ids, refs) = match &p.servers {
        Some(s) => {
            let (ids, refs) = servers(client, s).await?;
            (Some(ids), refs)
        }
        None => (None, Value::Null),
    };
    Ok((
        PartialBuilderConfig::Server(PartialServerBuilderConfig { server_ids }),
        refs,
    ))
}
fn proposed(
    current: Option<&BuilderConfig>,
    p: &PartialBuilderConfig,
) -> Result<BuilderConfig, AppError> {
    let PartialBuilderConfig::Server(p) = p else {
        return Err(unsupported());
    };
    let preserved = match current {
        Some(BuilderConfig::Server(c)) => c.server_ids.clone(),
        None => vec![],
        _ => return Err(unsupported()),
    };
    Ok(BuilderConfig::Server(ServerBuilderConfig {
        server_ids: p.server_ids.clone().unwrap_or(preserved),
    }))
}
fn result(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    current: Option<&BuilderConfig>,
    p: &PartialBuilderConfig,
) -> Result<ConfigurationResult, AppError> {
    let patch = common::partial_value(p)?;
    let after = proposed(current, p)?;
    let before = current.map_or(json!({}), |c| serde_json::to_value(c).expect("config"));
    Ok(ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: ConfigurationTarget {
            kind: "builder",
            id,
            name: common::bounded_text(client, name, 256),
        },
        preview_id: None,
        changes: common::changes(client, &before["params"], &patch["params"]),
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects: vec![
            SUPPORT,
            "Server order is Core fallback priority and is never sorted. An empty replacement leaves no Server available for a future build. This saves a definition, never provisions a host or submits a build.",
            "Core requires admin for Builder creation, Write for updates and Read/Attach on selected Servers. No credential elevation/fallback is used. Core's inspected Builder write hooks perform no execution, but changing an existing Builder's Servers can redirect automatically triggered Builds; supplying servers on update requires both local grants regardless of current policy.",
            "Shared ten-minute single-use previews bind private baseline and supplied Server configuration/trust. Apply rechecks references, submits at most one explicit Server-variant write, verifies its echo and independently reads the entire saved configuration. Core offers no CAS: a final-read/write race remains; uncertain writes are never retried.",
        ],
        definition: Some(common::DefinitionPreview::Builder(DefinitionPreview {
            before: current.map(summary),
            after: summary(&after),
            requires_operate_to_apply: current.is_some()
                && patch["params"].get("server_ids").is_some(),
            reference_validation: "Supplied visible Server definitions and trust; Core retains authoritative Read/Attach. Omitted Server list is preserved.",
        })),
    })
}
async fn finish(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<Builder, AppError>,
    patch: &Value,
    expected: (&BuilderConfig, &str, Option<&str>),
    op: &'static str,
) -> ConfigurationResult {
    let (config, name, id) = expected;
    let mut r = common::finish_save(r, response, patch, name, id, op, |r, c| {
        if let Some(common::DefinitionPreview::Builder(d)) = &mut r.definition {
            d.after = summary(c);
        }
    });
    if matches!(r.outcome, ConfigurationOutcome::Saved) {
        let raw = client
            .read_references(GetBuilder {
                builder: r.resource.id.clone().expect("saved"),
            })
            .await;
        r = common::verify_readback(
            r,
            raw,
            &serde_json::to_value(config).expect("config"),
            name,
            "GetBuilder",
        );
    }
    r
}
pub async fn preview_update(
    client: &CoreClient,
    input: PreviewInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    common::validate_payload(client, &input.patch)?;
    let bounded = client.bounded_evidence();
    let (_, b) = inventory::resolve_builder(&bounded, input.target.clone()).await?;
    server_config(&b)?;
    let (p, refs) = prepare(&bounded, &input.patch).await?;
    let r = result(client, Some(b.id.clone()), &b.name, Some(&b.config), &p)?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("builder_update", &input),
        baseline: common::resource_baseline("builder", &b),
        prepared: json!({"config":common::partial_value(&p)?,"references":refs}),
        result: r,
    })
}
pub async fn apply_update(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_operate(client, input.patch.servers.is_some(), "UpdateBuilder")?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("builder_update", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    let (_, b) = inventory::resolve_builder(&bounded, input.target).await?;
    server_config(&b)?;
    let (p, refs) = prepare(&bounded, &input.patch).await?;
    let patch = common::partial_value(&p)?;
    if entry.baseline != common::resource_baseline("builder", &b)
        || entry.prepared != json!({"config":patch,"references":refs})
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    let config = proposed(Some(&b.config), &p)?;
    let response = client
        .write_checked(UpdateBuilder {
            id: b.id.clone(),
            config: p,
        })
        .await?;
    Ok(finish(
        client,
        entry.result,
        response,
        &patch,
        (&config, &b.name, Some(&b.id)),
        "UpdateBuilder",
    )
    .await)
}
fn name(client: &CoreClient, input: &CreateInput) -> Result<String, AppError> {
    common::validate_payload(client, input)?;
    let name = to_general_name(&input.name);
    if input.name.len() > 256
        || input.name.chars().any(char::is_control)
        || name.is_empty()
        || name.len() > 128
        || crate::deletion::object_id(&name)
        || input.config.servers.as_ref().is_none_or(Vec::is_empty)
    {
        return Err(AppError::arguments(
            OP,
            "Creation requires a bounded name and kind=server with 1–16 ordered existing Servers",
        ));
    }
    Ok(name)
}
async fn absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_builder(
        client,
        DetailInput {
            id: None,
            name: Some(name.into()),
        },
    )
    .await
    {
        Err(e) if e.kind == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(AppError::new(ErrorKind::StalePreview, OP, None)),
        Err(e) => Err(e),
    }
}
pub async fn preview_create(
    client: &CoreClient,
    input: CreateInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let name = name(client, &input)?;
    let bounded = client.bounded_evidence();
    absent(&bounded, &name).await?;
    let (p, refs) = prepare(&bounded, &input.config).await?;
    let r = result(client, None, &name, None, &p)?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("builder_create", &input),
        baseline: json!({"kind":"builder_create","name":name}),
        prepared: json!({"config":common::partial_value(&p)?,"references":refs}),
        result: r,
    })
}
pub async fn apply_create(
    client: &CoreClient,
    input: CreateApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let submitted = CreateInput {
        name: input.name,
        config: input.config,
    };
    let name = name(client, &submitted)?;
    let entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("builder_create", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    absent(&bounded, &name).await?;
    let (p, refs) = prepare(&bounded, &submitted.config).await?;
    let patch = common::partial_value(&p)?;
    if entry.baseline != json!({"kind":"builder_create","name":name})
        || entry.prepared != json!({"config":patch,"references":refs})
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let config = proposed(None, &p)?;
    let response = client
        .write_checked(CreateBuilder {
            name: name.clone(),
            config: p,
        })
        .await?;
    Ok(finish(
        client,
        entry.result,
        response,
        &patch,
        (&config, &name, None),
        "CreateBuilder",
    )
    .await)
}

/// The typed transport independently rejects unsafe variants/conversions.
pub(crate) async fn guard_write(
    client: &CoreClient,
    op: &'static str,
    params: &Value,
) -> Result<(), AppError> {
    if !matches!(op, "CreateBuilder" | "UpdateBuilder") {
        return Ok(());
    }
    let c = &params["config"];
    if c["type"] != "Server" || !c["params"].is_object() {
        return Err(unsupported());
    }
    if let Some(ids) = c["params"].get("server_ids")
        && !ids.as_array().is_some_and(|v| {
            v.len() <= 16
                && v.iter()
                    .all(|s| s.as_str().is_some_and(crate::deletion::object_id))
        })
    {
        return Err(unsupported());
    }
    require_operate(
        client,
        op == "UpdateBuilder" && c["params"].get("server_ids").is_some(),
        op,
    )?;
    if op == "UpdateBuilder" {
        let id = params["id"]
            .as_str()
            .filter(|s| crate::deletion::object_id(s))
            .ok_or_else(unsupported)?;
        let (_, b) = inventory::resolve_builder(
            &client.bounded_evidence(),
            DetailInput {
                id: Some(id.into()),
                name: None,
            },
        )
        .await?;
        server_config(&b)?;
    }
    Ok(())
}
