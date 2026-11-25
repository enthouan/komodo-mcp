//! Cached container discovery and fresh inspection by server-bound immutable ID.
use std::collections::BTreeSet;

use komodo_client::{
    api::read::{InspectContainer, ListContainers},
    entities::docker::container::{ContainerStateStatusEnum, HealthStatusEnum},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use super::{
    Observation, Page, SCAN, finish, hex, image_reference, invalid, name, offsets, page, server,
    size, text,
};
use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
    inventory::DetailInput,
};

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListContainersInput {
    pub target: DetailInput,
    /// Include every cached state, including exited, paused and unknown.
    #[serde(default)]
    pub include_stopped: bool,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u64,
}

#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ContainerIdentity {
    #[schemars(regex(pattern = "^[0-9a-fA-F]{24}$"))]
    pub server_id: String,
    #[schemars(regex(pattern = "^[0-9a-f]{64}$"))]
    pub container_id: String,
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GetContainerInput {
    /// Copy the complete identity from the selected list_containers row.
    pub identity: ContainerIdentity,
}

#[derive(Serialize, JsonSchema)]
pub struct ContainerSummary {
    /// Null when the cache supplied insufficient evidence for exact inspection.
    pub identity: Option<ContainerIdentity>,
    pub name: Option<String>,
    pub image: Option<String>,
    pub state: Option<String>,
}

#[derive(Serialize, JsonSchema)]
pub struct ContainerDetail {
    pub identity: ContainerIdentity,
    pub name: Option<String>,
    pub state: Option<String>,
    pub exit_code: Option<i64>,
    pub health_status: Option<&'static str>,
    pub restart_count: Option<i64>,
}

fn state(value: ContainerStateStatusEnum) -> Option<String> {
    (value != ContainerStateStatusEnum::Empty).then(|| value.to_string())
}

pub async fn list(
    client: &CoreClient,
    input: ListContainersInput,
) -> Result<Observation<Page<ContainerSummary>>, AppError> {
    offsets(input.page, input.page_size)?;
    let client = client.bounded_evidence();
    let server = server(&client, input.target).await?;
    let mut inventory = client
        .read(ListContainers {
            server: server.id.clone(),
        })
        .await?;
    let observed = inventory.len();
    inventory.truncate(SCAN);
    inventory.sort_by(|a, b| (&a.name, &a.id).cmp(&(&b.name, &b.id)));

    let server_id_visible = client.redact(&server.id) == server.id;
    let mut seen = BTreeSet::new();
    let mut rows = Vec::new();
    for item in inventory {
        if item.server_id.as_ref().is_some_and(|id| id != &server.id) {
            return Err(invalid());
        }
        let id = item.id.filter(|id| hex(id));
        if id.as_ref().is_some_and(|id| !seen.insert(id.clone())) {
            return Err(invalid());
        }
        if !input.include_stopped && item.state != ContainerStateStatusEnum::Running {
            continue;
        }
        rows.push(ContainerSummary {
            identity: id
                .filter(|id| server_id_visible && client.redact(id) == *id)
                .map(|container_id| ContainerIdentity {
                    server_id: server.id.clone(),
                    container_id,
                }),
            name: name(&item.name).then(|| text(&client, &item.name, 128)),
            image: item
                .image
                .filter(|v| image_reference(v))
                .map(|v| text(&client, &v, 512)),
            state: state(item.state),
        });
    }
    finish(
        &client,
        server,
        page(rows, input.page, input.page_size, observed)?,
        (
            "core_cache",
            "Core supplies a cached inventory with unknown age. An empty page can reflect the filter, page offset, empty inventory, missing cache or a suppressed poll failure. observed_count includes all states before local bounds and filtering. A null identity cannot be inspected.",
        ),
    )
}

pub async fn inspect(
    client: &CoreClient,
    input: GetContainerInput,
) -> Result<Observation<ContainerDetail>, AppError> {
    let identity = input.identity;
    if !hex(&identity.container_id) {
        return Err(AppError::arguments(
            "InspectContainer",
            "Use the full lowercase 64-character container ID returned by list_containers.",
        ));
    }
    let client = client.bounded_evidence();
    let server = server(
        &client,
        DetailInput {
            id: Some(identity.server_id.clone()),
            name: None,
        },
    )
    .await?;
    let full = client
        .read(InspectContainer {
            server: server.id.clone(),
            container: identity.container_id.clone(),
        })
        .await?;
    let invalid_identity = |message| AppError {
        message,
        ..AppError::new(ErrorKind::InvalidResponse, "InspectContainer", None)
    };
    let id = full.id.as_deref().filter(|id| hex(id)).ok_or_else(|| {
        invalid_identity("Docker omitted a valid full container ID. The requested identity could not be verified.")
    })?;
    if id != identity.container_id {
        return Err(invalid_identity(
            "Docker returned a different container ID. List containers again to identify the current container.",
        ));
    }
    if client.redact(id) != id || client.redact(&server.id) != server.id {
        return Err(invalid_identity(
            "The fresh identity could not be represented in the projected response.",
        ));
    }
    let detail = ContainerDetail {
        identity: ContainerIdentity {
            server_id: server.id.clone(),
            container_id: identity.container_id,
        },
        name: full
            .name
            .as_deref()
            .map(|v| v.strip_prefix('/').unwrap_or(v))
            .filter(|v| name(v))
            .map(|v| text(&client, v, 128)),
        state: full.state.as_ref().and_then(|s| state(s.status)),
        exit_code: full.state.as_ref().and_then(|s| s.exit_code),
        health_status: full
            .state
            .and_then(|s| s.health)
            .and_then(|h| match h.status {
                HealthStatusEnum::Empty => None,
                HealthStatusEnum::None => Some("none"),
                HealthStatusEnum::Starting => Some("starting"),
                HealthStatusEnum::Healthy => Some("healthy"),
                HealthStatusEnum::Unhealthy => Some("unhealthy"),
            }),
        restart_count: full.restart_count.filter(|n| *n >= 0),
    };
    finish(
        &client,
        server,
        detail,
        (
            "fresh_inspect",
            "Fresh Docker inspection through Core and Periphery for the selected Server and full container ID. Unavailable diagnostics are null.",
        ),
    )
}
