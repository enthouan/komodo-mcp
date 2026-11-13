//! Server records, with saved configuration separate from cached connectivity.
use std::time::Instant;

use komodo_client::{
    api::write::{CreateServer, UpdateServer},
    entities::{
        server::{PartialServerConfig, Server, ServerConfig, ServerListItem},
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
    inventory::{self, DetailInput},
};

const OP: &str = "server_configuration_edit";

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ServerPatch {
    /// Empty selects Periphery-to-Core connections. Never contains credentials.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub address: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub insecure_tls: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub region: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub stats_monitoring: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub send_unreachable_alerts: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub send_cpu_alerts: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub send_mem_alerts: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub send_disk_alerts: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub send_version_mismatch_alerts: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<String>")]
    pub ignore_mounts: Option<Vec<String>>,
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
    #[schemars(schema_with = "create_config_schema")]
    pub config: ServerPatch,
    /// Optional initial trust, never an update/rotation of existing trust.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub initial_public_key: Option<String>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApplyInput {
    pub name: String,
    #[schemars(schema_with = "create_config_schema")]
    pub config: ServerPatch,
    #[serde(default, deserialize_with = "present")]
    #[schemars(with = "String")]
    pub initial_public_key: Option<String>,
    pub preview_id: String,
}
fn create_config_schema(g: &mut schemars::SchemaGenerator) -> schemars::Schema {
    schemars::json_schema!({"allOf":[g.subschema_for::<ServerPatch>()],"required":["address","enabled","insecure_tls"]})
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub connection_direction: &'static str,
    pub address_configured: bool,
    pub enabled: bool,
    pub insecure_tls: bool,
    pub region: String,
    pub public_key_configured: bool,
    pub auto_prune: bool,
    pub auto_rotate_keys: bool,
    pub stats_monitoring: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct ConnectionObservation {
    pub core_reported_state: Option<String>,
    pub connection_verified: bool,
    pub observed_at: &'static str,
    pub error: Option<AppError>,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub connectivity: Option<ConnectionObservation>,
    pub requires_operate_to_apply: bool,
    pub existing_trust_preserved: bool,
}
fn invalid() -> AppError {
    AppError::arguments(
        OP,
        "Use documented typed fields, an empty address for inbound connections or a bounded ws/wss/http/https URL without credentials/query/fragment, and supported empty values for clears",
    )
}
impl ServerPatch {
    fn validate(&self, client: &CoreClient) -> Result<(), AppError> {
        common::validate_payload(client, self)?;
        if self.address.as_ref().is_some_and(|s| !valid_address(s))
            || self
                .region
                .as_ref()
                .is_some_and(|s| s.len() > 256 || s.chars().any(char::is_control))
            || self.ignore_mounts.as_ref().is_some_and(|v| {
                v.len() > 32
                    || v.iter()
                        .any(|s| s.is_empty() || s.len() > 256 || s.chars().any(char::is_control))
            })
        {
            return Err(invalid());
        }
        Ok(())
    }
    fn connection_supplied(&self) -> bool {
        self.address.is_some() || self.enabled.is_some() || self.insecure_tls.is_some()
    }
    fn partial(&self) -> PartialServerConfig {
        PartialServerConfig {
            address: self.address.clone(),
            insecure_tls: self.insecure_tls,
            enabled: self.enabled,
            region: self.region.clone(),
            stats_monitoring: self.stats_monitoring,
            send_unreachable_alerts: self.send_unreachable_alerts,
            send_cpu_alerts: self.send_cpu_alerts,
            send_mem_alerts: self.send_mem_alerts,
            send_disk_alerts: self.send_disk_alerts,
            send_version_mismatch_alerts: self.send_version_mismatch_alerts,
            ignore_mounts: self.ignore_mounts.clone(),
            ..Default::default()
        }
    }
}
fn valid_address(s: &str) -> bool {
    if s.is_empty() {
        return true;
    }
    if s.len() > 2048
        || s.chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\')
    {
        return false;
    }
    reqwest::Url::parse(s).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https" | "ws" | "wss")
            && u.host_str().is_some()
            && u.username().is_empty()
            && u.password().is_none()
            && u.query().is_none()
            && u.fragment().is_none()
            && u.port() != Some(0)
    })
}
pub(crate) fn require_connection_operate(
    client: &CoreClient,
    supplied: bool,
) -> Result<(), AppError> {
    if supplied && !client.capabilities().allows(Capability::Operate) {
        return Err(AppError {
            field: Some("KOMODO_ALLOW_OPERATE"),
            correction: Some(
                "Applying supplied Server connection fields or creating an enabled Server requires both local grants: Core reconnects/refreshes and existing background prune/key policies can follow the target. No request was submitted.",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, "UpdateServer", None)
        });
    }
    Ok(())
}
fn baseline(v: &Server) -> Value {
    json!({"resource":common::resource_baseline("server", v),"public_key":v.info.public_key})
}
fn summary(client: &CoreClient, c: &ServerConfig, key: &str) -> DefinitionSummary {
    DefinitionSummary {
        connection_direction: if c.address.is_empty() {
            "periphery_to_core"
        } else {
            "core_to_periphery"
        },
        address_configured: !c.address.is_empty(),
        enabled: c.enabled,
        insecure_tls: c.insecure_tls,
        region: common::bounded_text(client, &c.region, 256),
        public_key_configured: !key.is_empty(),
        auto_prune: c.auto_prune,
        auto_rotate_keys: c.auto_rotate_keys,
        stats_monitoring: c.stats_monitoring,
    }
}
fn proposed(
    current: Option<&ServerConfig>,
    partial: &PartialServerConfig,
) -> Result<ServerConfig, AppError> {
    let mut v = serde_json::to_value(current.cloned().unwrap_or_default()).expect("config");
    v.as_object_mut().expect("config object").extend(
        common::partial_value(partial)?
            .as_object()
            .expect("partial")
            .clone(),
    );
    serde_json::from_value(v).map_err(|_| invalid())
}
fn observation(listed: &ServerListItem, at: &'static str) -> ConnectionObservation {
    let state = serde_json::to_value(listed.info.state)
        .expect("state")
        .as_str()
        .unwrap_or("unknown")
        .to_string();
    let error = if state == "NotOk" {
        Some(AppError {
            message: "Core reports this Server is not connected. Verify the saved address, network, TLS setting and explicitly configured Periphery public key; no key acceptance, rotation, credential fallback or provisioning was attempted.",
            ..AppError::new(ErrorKind::Unreachable, "ServerConnectivity", None)
        })
    } else {
        None
    };
    ConnectionObservation {
        core_reported_state: Some(state),
        connection_verified: false,
        observed_at: at,
        error,
    }
}
fn result(
    client: &CoreClient,
    target: ConfigurationTarget,
    current: Option<&ServerConfig>,
    partial: &PartialServerConfig,
    key: &str,
    connectivity: Option<ConnectionObservation>,
    requires_operate: bool,
) -> Result<ConfigurationResult, AppError> {
    let patch = common::partial_value(partial)?;
    let old = current.map_or(json!({}), |c| serde_json::to_value(c).expect("config"));
    let changes = common::changes(client, &old, &patch);
    let after = proposed(current, partial)?;
    if !changes.is_empty() && after.enabled && key.trim().is_empty() {
        return Err(AppError::arguments(
            OP,
            "Enabled Server writes require an explicit pinned public key. Create with initial_public_key, or stage disabled and establish trust separately in Core before enabling. Existing keys are never accepted or rotated by these tools.",
        ));
    }
    Ok(ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: target,
        preview_id: None,
        changes,
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects: vec![
            "MCP submits one typed Server record write, with no host provisioning, shell, container lifecycle, network creation, public-key update or key-rotation request.",
            "Core 2.3.2 refreshes its cache after create/update: enabled records can reconnect, authenticate, poll system/Docker status and inspect attached Repo state. Disabled records disconnect and mark observations unknown. This is separate from saving the definition; connection failure can coexist with a verified save.",
            "Connectivity is Core-reported cached evidence at the indicated time. Even an Ok state does not establish a new successful connection to the saved address. Raw connection errors, addresses, credentials and keys are not returned.",
            "Creation explicitly disables automatic image pruning and automatic key rotation instead of inheriting Core's true defaults. Updates preserve existing policies. Connection-field applies require both local grants independently of current policy, because policies and connections can race.",
            "Enabled writes require an explicitly pinned Periphery public key; Core otherwise skips outbound key validation. Initial trust can be supplied only at creation. Existing public keys are included in the private stale baseline and are never replaced or rotated. A key mismatch requires separate deliberate trust management in Core; no fallback is attempted.",
            "Shared ten-minute single-use previews revalidate raw definitions and pinned trust before at most one write. Core has no conditional write or invocation idempotency; another writer or policy can race after final reads. Never automatically retry an uncertain result.",
        ],
        definition: Some(common::DefinitionPreview::Server(DefinitionPreview {
            before: current.map(|c| summary(client, c, key)),
            after: summary(client, &after, key),
            connectivity,
            requires_operate_to_apply: requires_operate,
            existing_trust_preserved: current.is_some(),
        })),
    })
}
async fn observe_saved(client: &CoreClient, mut r: ConfigurationResult) -> ConfigurationResult {
    if !matches!(r.outcome, ConfigurationOutcome::Saved) {
        return r;
    }
    let id = r.resource.id.clone().expect("verified saved identity");
    let observation = match inventory::resolve_server(
        &client.bounded_evidence(),
        DetailInput {
            id: Some(id),
            name: None,
        },
    )
    .await
    {
        Ok((listed, _)) => observation(&listed, "after_save"),
        Err(error) => ConnectionObservation {
            core_reported_state: None,
            connection_verified: false,
            observed_at: "after_save",
            error: Some(error),
        },
    };
    if let Some(common::DefinitionPreview::Server(d)) = &mut r.definition {
        d.connectivity = Some(observation);
    }
    r
}
fn finish(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<Server, AppError>,
    patch: &Value,
    expected: (&str, Option<&str>, &str),
    op: &'static str,
) -> ConfigurationResult {
    let (name, id, key) = expected;
    let response = response.and_then(|v| {
        if v.info.public_key == key {
            Ok(v)
        } else {
            Err(AppError::new(ErrorKind::InvalidResponse, op, None))
        }
    });
    common::finish_save(r, response, patch, name, id, op, |r, c| {
        if let Some(common::DefinitionPreview::Server(d)) = &mut r.definition {
            d.after = summary(client, c, key);
        }
    })
}
pub async fn preview_update(
    client: &CoreClient,
    input: PreviewInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    input.patch.validate(client)?;
    let (listed, current) =
        inventory::resolve_server(&client.bounded_evidence(), input.target.clone()).await?;
    let partial = input.patch.partial();
    let r = result(
        client,
        ConfigurationTarget {
            kind: "server",
            id: Some(current.id.clone()),
            name: common::bounded_text(client, &current.name, 256),
        },
        Some(&current.config),
        &partial,
        &current.info.public_key,
        Some(observation(&listed, "preview_time")),
        input.patch.connection_supplied(),
    )?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("server_update", &input),
        baseline: baseline(&current),
        prepared: common::partial_value(&partial)?,
        result: r,
    })
}
pub async fn apply_update(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_connection_operate(client, input.patch.connection_supplied())?;
    input.patch.validate(client)?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("server_update", &submitted),
    )?;
    let (_, current) = inventory::resolve_server(&client.bounded_evidence(), input.target).await?;
    let partial = input.patch.partial();
    let patch = common::partial_value(&partial)?;
    if baseline(&current) != entry.baseline || patch != entry.prepared {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    entry.result.preview_id = None;
    if entry.result.changes.is_empty() {
        entry.result.outcome = ConfigurationOutcome::NoOp;
        entry.result.saved_configuration_verified = true;
        return Ok(entry.result);
    }
    let response = client
        .write(UpdateServer {
            id: current.id.clone(),
            config: partial,
        })
        .await;
    Ok(observe_saved(
        client,
        finish(
            client,
            entry.result,
            response,
            &patch,
            (&current.name, Some(&current.id), &current.info.public_key),
            "UpdateServer",
        ),
    )
    .await)
}
fn create(
    client: &CoreClient,
    input: &CreateInput,
) -> Result<(String, PartialServerConfig), AppError> {
    input.config.validate(client)?;
    common::validate_payload(client, input)?;
    let name = to_general_name(&input.name);
    if input.name.chars().any(|c| c.is_control() && c != '\n')
        || input.name.len() > 256
        || name.is_empty()
        || name.len() > 128
        || (name.len() == 24 && name.bytes().all(|b| b.is_ascii_hexdigit()))
        || input.config.address.is_none()
        || input.config.enabled.is_none()
        || input.config.insecure_tls.is_none()
        || input
            .initial_public_key
            .as_ref()
            .is_some_and(|s| s.trim().is_empty() || s.len() > 4096 || s.contains('\0'))
    {
        return Err(AppError::arguments(
            "CreateServer",
            "Provide a bounded nonempty general name (no ObjectId name), explicit address/enabled/insecure_tls, and an optional nonblank initial public key of at most 4096 bytes. Enabled creation requires an explicitly pinned key.",
        ));
    }
    let mut partial = input.config.partial();
    partial.auto_prune = Some(false);
    partial.auto_rotate_keys = Some(false);
    Ok((name, partial))
}
async fn require_absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_server(
        client,
        DetailInput {
            id: None,
            name: Some(name.into()),
        },
    )
    .await
    {
        Err(e) if e.kind == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(AppError::new(ErrorKind::StalePreview, "CreateServer", None)),
        Err(e) => Err(e),
    }
}
pub async fn preview_create(
    client: &CoreClient,
    input: CreateInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let (name, partial) = create(client, &input)?;
    let key = input.initial_public_key.as_deref().unwrap_or_default();
    let mut r = result(
        client,
        ConfigurationTarget {
            kind: "server",
            id: None,
            name: common::bounded_text(client, &name, 256),
        },
        None,
        &partial,
        key,
        None,
        input.config.enabled == Some(true),
    )?;
    if input.initial_public_key.is_some() {
        r.changes.extend(common::changes(
            client,
            &json!({}),
            &json!({"initial_public_key":key}),
        ));
    }
    r.effects.push("Creation checks bounded visible name absence only; Core enforces global uniqueness and admin or enabled create_server_permissions with non-admin creation policy. Hidden names and concurrent creators can reject submission. Initial public key is sent only to CreateServer and echoed trust is verified privately.");
    require_absent(&client.bounded_evidence(), &name).await?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("server_create", &input),
        baseline: json!({"kind":"server_create","name":name}),
        prepared: common::partial_value(&partial)?,
        result: r,
    })
}
pub async fn apply_create(
    client: &CoreClient,
    input: CreateApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_connection_operate(client, input.config.enabled == Some(true))?;
    let preview_id = input.preview_id;
    let input = CreateInput {
        name: input.name,
        config: input.config,
        initial_public_key: input.initial_public_key,
    };
    let (name, partial) = create(client, &input)?;
    // The exact initial trust is part of the private input binding.
    let entry = client
        .previews()
        .take(&preview_id, &common::bound_input("server_create", &input))?;
    let patch = common::partial_value(&partial)?;
    if entry.baseline != json!({"kind":"server_create","name":name}) || entry.prepared != patch {
        return Err(AppError::new(ErrorKind::InvalidPreview, OP, None));
    }
    require_absent(&client.bounded_evidence(), &name).await?;
    let key = input.initial_public_key.as_deref().unwrap_or_default();
    let response = client
        .write(CreateServer {
            name: name.clone(),
            config: partial,
            public_key: input.initial_public_key.clone(),
        })
        .await;
    Ok(observe_saved(
        client,
        finish(
            client,
            entry.result,
            response,
            &patch,
            (&name, None, key),
            "CreateServer",
        ),
    )
    .await)
}
