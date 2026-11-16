//! Private routing and effective-authority proof. Never serialized to MCP.
use komodo_client::{
    api::read::{
        GetDeployment, GetPermission, GetServer, GetStack, InspectContainer,
        InspectDeploymentContainer, InspectStackContainer, ListStackServices,
    },
    entities::{ResourceTarget, permission::PermissionLevel},
};
use serde_json::{Value, json};

use super::{ContainerSelector, OP, Target, invalid, require};
use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
    export,
    inventory::DetailInput,
};

pub(super) struct Pinned {
    pub spec: Target,
    pub target: komodo_client::entities::terminal::TerminalTarget,
    pub baseline: Value,
    pub container: Option<Value>,
    pub container_name: Option<String>,
    pub caller_id: String,
    pub resource_id: String,
}

fn stale() -> AppError {
    AppError::new(ErrorKind::StalePreview, OP, None)
}
fn unsupported() -> AppError {
    AppError::new(ErrorKind::UnsupportedTarget, OP, None)
}
fn by_id(id: &str) -> DetailInput {
    DetailInput {
        id: Some(id.into()),
        name: None,
    }
}
pub(super) fn docker_id(id: &str) -> bool {
    id.len() == 64
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
pub(super) fn docker_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 128
        && name
            .bytes()
            .next()
            .is_some_and(|b| b.is_ascii_alphanumeric())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
pub(super) fn service_name(name: &str) -> bool {
    docker_name(name)
}

fn inspect(raw: Value) -> Result<Value, AppError> {
    let id = raw["Id"]
        .as_str()
        .filter(|s| docker_id(s))
        .ok_or_else(invalid)?;
    let name = raw["Name"]
        .as_str()
        .and_then(|s| s.strip_prefix('/'))
        .filter(|s| docker_name(s))
        .ok_or_else(invalid)?;
    let created = raw["Created"]
        .as_str()
        .filter(|s| !s.is_empty() && s.len() <= 128)
        .ok_or_else(invalid)?;
    let labels = &raw["Config"]["Labels"];
    // Pinned Core serializes ContainerConfig.labels as a nonoptional map.
    // Missing/null Config or Labels is incomplete evidence, never proof that
    // this container lacks Swarm task/service identity labels.
    if !labels
        .as_object()
        .is_some_and(|labels| labels.values().all(Value::is_string))
    {
        return Err(invalid());
    }
    if labels
        .as_object()
        .is_some_and(|m| m.keys().any(|k| k.starts_with("com.docker.swarm.")))
    {
        return Err(unsupported());
    }
    if raw["State"]["Running"] != true {
        return Err(unsupported());
    }
    // Exclude volatile stats, environment, mounts and raw application configuration.
    Ok(json!({"id":id,"name":name,"created":created,"labels":labels}))
}

pub(super) async fn prepare(
    client: &CoreClient,
    spec: Target,
    live: bool,
) -> Result<Pinned, AppError> {
    use komodo_client::entities::terminal::TerminalTarget as T;
    require(client)?;
    let client = client.bounded_evidence();
    let (kind, selector) = match &spec {
        Target::Server { target } | Target::Container { server: target, .. } => {
            (export::Kind::Server, target.clone())
        }
        Target::Stack { target, .. } => (export::Kind::Stack, target.clone()),
        Target::Deployment { target } => (export::Kind::Deployment, target.clone()),
    };
    export::validate_selector(&selector)?;
    let selected = export::select(
        &client,
        export::Target {
            kind,
            target: selector,
        },
    )
    .await?;
    let id = selected.id;
    let parent = match &spec {
        Target::Server { .. } | Target::Container { .. } => {
            client
                .read_references(GetServer { server: id.clone() })
                .await?
        }
        Target::Stack { .. } => {
            client
                .read_references(GetStack { stack: id.clone() })
                .await?
        }
        Target::Deployment { .. } => {
            client
                .read_references(GetDeployment {
                    deployment: id.clone(),
                })
                .await?
        }
    };
    if crate::deletion::raw_id(&parent).map_err(|_| invalid())? != id
        || parent["name"] != selected.name
        || !parent["config"].is_object()
        || !parent["template"].is_boolean()
    {
        return Err(invalid());
    }
    if parent["template"] != false {
        return Err(unsupported());
    }
    let is_server = matches!(spec, Target::Server { .. } | Target::Container { .. });
    let server_id = if is_server {
        id.clone()
    } else {
        let swarm = parent["config"]["swarm_id"].as_str().ok_or_else(invalid)?;
        let server = parent["config"]["server_id"].as_str().ok_or_else(invalid)?;
        if !swarm.is_empty() || !crate::deletion::object_id(server) {
            return Err(unsupported());
        }
        server.into()
    };
    let server = if is_server {
        parent.clone()
    } else {
        client
            .read_references(GetServer {
                server: server_id.clone(),
            })
            .await?
    };
    if crate::deletion::raw_id(&server).map_err(|_| invalid())? != server_id
        || !server["template"].is_boolean()
        || !server["config"]["enabled"].is_boolean()
    {
        return Err(invalid());
    }
    if server["template"] != false || server["config"]["enabled"] != true {
        return Err(unsupported());
    }
    let caller = client.current_caller().await?;
    if caller["enabled"] != true {
        return Err(AppError::new(ErrorKind::PermissionDenied, OP, None));
    }
    let resource_target = match &spec {
        Target::Server { .. } | Target::Container { .. } => ResourceTarget::Server(id.clone()),
        Target::Stack { .. } => ResourceTarget::Stack(id.clone()),
        Target::Deployment { .. } => ResourceTarget::Deployment(id.clone()),
    };
    let permission = client
        .read_references(GetPermission {
            target: resource_target,
        })
        .await?;
    let level = permission.get("level").unwrap_or(&permission);
    let level: PermissionLevel = serde_json::from_value(level.clone()).map_err(|_| invalid())?;
    let specifics = permission.get("specific").and_then(Value::as_array);
    let needs_inspect = !matches!(spec, Target::Server { .. });
    // A scalar permission has no specifics. Write/Execute never manufacture Terminal.
    if level == PermissionLevel::None
        || !specifics.is_some_and(|s| s.iter().any(|v| v == "Terminal"))
        || (needs_inspect && !specifics.is_some_and(|s| s.iter().any(|v| v == "Inspect")))
    {
        return Err(AppError {
            correction: Some(
                "The same enabled caller needs Core Read + Terminal on the original target, and Inspect for container identity. Execute/Write alone cannot substitute. Parent targets also require the saved Server definition to be readable.",
            ),
            ..AppError::new(ErrorKind::PermissionDenied, OP, None)
        });
    }
    let baseline = json!({"parent":parent,"server":server,"caller":caller,"permission":permission});
    if baseline.to_string().len() > 256 * 1024 {
        return Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None));
    }
    let mut container = None;
    let mut container_name = None;
    let (spec, target) = match spec {
        Target::Server { .. } => (
            Target::Server { target: by_id(&id) },
            T::Server {
                server: Some(id.clone()),
            },
        ),
        Target::Container {
            container: selector,
            ..
        } => {
            let locator = match &selector {
                ContainerSelector::Id { id } => id,
                ContainerSelector::Name { name } => name,
            };
            let proof = if live {
                Some(inspect(
                    client
                        .read_references(InspectContainer {
                            server: server_id.clone(),
                            container: locator.clone(),
                        })
                        .await?,
                )?)
            } else {
                None
            };
            if let Some(p) = &proof
                && match &selector {
                    ContainerSelector::Id { id } => p["id"] != *id,
                    ContainerSelector::Name { name } => p["name"] != *name,
                }
            {
                return Err(stale());
            }
            // The initial resolution pins a name selector to its complete immutable ID.
            let cid = proof
                .as_ref()
                .and_then(|p| p["id"].as_str())
                .unwrap_or(locator)
                .to_owned();
            container_name = Some(cid.clone());
            container = proof;
            (
                Target::Container {
                    server: by_id(&server_id),
                    container: ContainerSelector::Id { id: cid.clone() },
                },
                T::Container {
                    server: server_id.clone(),
                    container: cid,
                },
            )
        }
        Target::Stack { service, .. } => {
            if live {
                let services = client
                    .read_references(ListStackServices { stack: id.clone() })
                    .await?;
                let services = services
                    .as_array()
                    .filter(|s| s.len() <= 512)
                    .ok_or_else(invalid)?;
                let matches: Vec<_> = services
                    .iter()
                    .filter(|s| s["service"] == service)
                    .collect();
                if matches.len() != 1 {
                    return Err(AppError::new(ErrorKind::AmbiguousTarget, OP, None));
                }
                let cached = matches[0];
                if cached["stack_id"] != id || !cached["swarm_service"].is_null() {
                    return Err(unsupported());
                }
                let proof = inspect(
                    client
                        .read_references(InspectStackContainer {
                            stack: id.clone(),
                            service: service.clone(),
                        })
                        .await?,
                )?;
                if cached["container"]["id"] != proof["id"]
                    || cached["container"]["name"] != proof["name"]
                {
                    return Err(stale());
                }
                let parent = &baseline["parent"];
                let project = parent["info"]["deployed_project_name"]
                    .as_str()
                    .or_else(|| {
                        parent["config"]["project_name"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                    })
                    .or_else(|| parent["name"].as_str())
                    .ok_or_else(invalid)?;
                if proof["labels"]["com.docker.compose.service"] != service
                    || proof["labels"]["com.docker.compose.project"] != project
                {
                    return Err(stale());
                }
                container_name = proof["name"].as_str().map(String::from);
                container = Some(proof);
            }
            (
                Target::Stack {
                    target: by_id(&id),
                    service: service.clone(),
                },
                T::Stack {
                    stack: id.clone(),
                    service: Some(service),
                },
            )
        }
        Target::Deployment { .. } => {
            if live {
                let proof = inspect(
                    client
                        .read_references(InspectDeploymentContainer {
                            deployment: id.clone(),
                        })
                        .await?,
                )?;
                let parent = &baseline["parent"];
                let name = parent["info"]["deployed_name"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        parent["config"]["custom_name"]
                            .as_str()
                            .filter(|s| !s.is_empty())
                    })
                    .or_else(|| parent["name"].as_str())
                    .ok_or_else(invalid)?;
                if proof["name"] != name {
                    return Err(stale());
                }
                container_name = Some(name.into());
                container = Some(proof);
            }
            (
                Target::Deployment { target: by_id(&id) },
                T::Deployment {
                    deployment: id.clone(),
                },
            )
        }
    };
    Ok(Pinned {
        spec,
        target,
        caller_id: caller["id"].as_str().ok_or_else(invalid)?.into(),
        resource_id: id,
        baseline,
        container,
        container_name,
    })
}

impl Pinned {
    pub async fn check(&self, client: &CoreClient, live: bool) -> Result<(), AppError> {
        let fresh = prepare(client, self.spec.clone(), live).await?;
        if fresh.baseline != self.baseline
            || fresh.target != self.target
            || (live && fresh.container != self.container)
        {
            return Err(stale());
        }
        Ok(())
    }
    pub fn scope(&self) -> komodo_client::entities::terminal::TerminalTarget {
        use komodo_client::entities::terminal::TerminalTarget as T;
        match &self.target {
            T::Stack { stack, .. } => T::Stack {
                stack: stack.clone(),
                service: None,
            },
            target => target.clone(),
        }
    }
}
