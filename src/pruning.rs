//! One reviewed Docker category on one stable Server; no dry-run or retry claim.
use std::time::Instant;

use komodo_client::{
    api::{
        execute::*,
        read::{GetPermission, GetServer, ListContainers},
    },
    entities::{Operation, ResourceTarget, permission::PermissionLevel},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::{
    capabilities::Capability,
    client::CoreClient,
    configuration::{self as common, Preview},
    error::{AppError, ErrorKind},
    export,
    inventory::DetailInput,
    operations::{
        self, ConfiguredEffects, OperationResource, RequestedScope, SubmissionReceipt,
        WorkloadKind, WorkloadOperation,
    },
};
const OP: &str = "docker_prune";
const LIMITS: &str = "One stable Server and one category only. Core offers no exact atomic dry run, named-object selection, expected daemon ID or compare-and-swap. Cached inventory is supporting evidence only; the eligible object set and live connection can change before execution. All unused objects in the selected category can be removed, including shared application data. No rollback or automatic replay; inspect a known update or reconcile read-only after uncertainty.";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Image,
    Container,
    Network,
    Volume,
    System,
}
impl Category {
    pub(crate) fn core(self) -> Operation {
        match self {
            Self::Image => Operation::PruneImages,
            Self::Container => Operation::PruneContainers,
            Self::Network => Operation::PruneNetworks,
            Self::Volume => Operation::PruneVolumes,
            Self::System => Operation::PruneSystem,
        }
    }
    pub(crate) fn operation(self) -> &'static str {
        match self {
            Self::Image => "PruneImages",
            Self::Container => "PruneContainers",
            Self::Network => "PruneNetworks",
            Self::Volume => "PruneVolumes",
            Self::System => "PruneSystem",
        }
    }
    fn effect(self) -> &'static str {
        match self {
            Self::Image => {
                "Remove all unused Docker images, including tagged images not referenced by a container; not only dangling images."
            }
            Self::Container => {
                "Remove all stopped Docker containers on this Server's daemon; this is not deletion of one named container."
            }
            Self::Network => {
                "Remove unused Docker networks according to the daemon's current eligibility rules; no named-network filter or frozen object set."
            }
            Self::Volume => {
                "Remove all unused local Docker volumes, including unused named and anonymous volumes. Unused named volumes can contain application data."
            }
            Self::System => {
                "Remove stopped containers, unused networks, all unused images, unused build cache and eligible unused anonymous volumes. Includes volume effects; unused named-volume removal is not established by system prune. Docker version/eligibility remains authoritative."
            }
        }
    }
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub target: DetailInput,
    pub category: Category,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: DetailInput,
    pub category: Category,
    pub preview_id: String,
    pub acknowledge_destructive_effects: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct Server {
    pub id: String,
    pub name: String,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct InventoryEvidence {
    pub category: Category,
    pub cached_observed_count: Option<usize>,
    pub scan_incomplete: bool,
    pub error: Option<AppError>,
    pub evidence_limit: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct PruneResult {
    pub outcome: &'static str,
    pub server: Server,
    pub category: Category,
    pub destructive_effect: &'static str,
    pub core_operation: &'static str,
    pub supporting_inventory: Vec<InventoryEvidence>,
    pub preview_id: Option<String>,
    pub prune_submissions: u8,
    pub receipt: Option<SubmissionReceipt>,
    pub exact_dry_run_available: bool,
    pub evidence_limit: &'static str,
}
pub(crate) fn require(client: &CoreClient) -> Result<(), AppError> {
    if !client.capabilities().allows(Capability::Prune) {
        return Err(AppError {
            field: Some("KOMODO_ALLOW_PRUNE"),
            correction: Some(
                "Pruning requires KOMODO_ALLOW_PRUNE=true and KOMODO_ALLOW_OPERATE=true. Configuration-edit does not enable it. No Core request was submitted.",
            ),
            ..AppError::new(ErrorKind::CapabilityDisabled, OP, None)
        });
    }
    Ok(())
}
fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, OP, None)
}
fn stale() -> AppError {
    AppError::new(ErrorKind::StalePreview, OP, None)
}
fn bound(v: &Value) -> Result<(), AppError> {
    if v.to_string().len() > 256 * 1024 {
        return Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None));
    }
    Ok(())
}
fn label(client: &CoreClient, name: &str) -> String {
    if name.contains("://") || crate::logs::redact_log(client, name) != name {
        client.redaction_marker().into()
    } else {
        common::bounded_text(client, name, 128)
    }
}
struct Prepared {
    server: Server,
    baseline: Value,
}
async fn prepare(client: &CoreClient, target: DetailInput) -> Result<Prepared, AppError> {
    require(client)?;
    export::validate_selector(&target)?;
    let caller = client.current_caller().await?;
    if caller["enabled"] != true {
        return Err(AppError::new(ErrorKind::PermissionDenied, OP, None));
    }
    let selected = export::select(
        client,
        export::Target {
            kind: export::Kind::Server,
            target,
        },
    )
    .await?;
    let raw = client
        .read_references(GetServer {
            server: selected.id.clone(),
        })
        .await?;
    if crate::deletion::raw_id(&raw).map_err(|_| invalid())? != selected.id
        || raw["name"] != selected.name
        || !raw["template"].is_boolean()
        || !raw["config"].is_object()
        || !raw["config"]["enabled"].is_boolean()
    {
        return Err(invalid());
    }
    if raw["template"] != false || raw["config"]["enabled"] != true {
        return Err(AppError {
            correction: Some(
                "Select one enabled, non-template Server; never substitute a different target after failure.",
            ),
            ..AppError::new(ErrorKind::UnsupportedTarget, OP, None)
        });
    }
    let permission = client
        .read_references(GetPermission {
            target: ResourceTarget::Server(selected.id.clone()),
        })
        .await?;
    let value = if permission.is_string() {
        &permission
    } else {
        if !permission["specific"].is_array() {
            return Err(invalid());
        }
        &permission["level"]
    };
    let level: PermissionLevel = serde_json::from_value(value.clone()).map_err(|_| invalid())?;
    if !matches!(level, PermissionLevel::Execute | PermissionLevel::Write) {
        return Err(AppError {
            correction: Some(
                "Use the same enabled credential with Core Execute on this exact Server. No administrator fallback is used.",
            ),
            ..AppError::new(ErrorKind::PermissionDenied, OP, None)
        });
    }
    let baseline = json!({"server":raw,"caller":caller,"permission":permission});
    bound(&baseline)?;
    Ok(Prepared {
        server: Server {
            id: selected.id,
            name: label(client, &selected.name),
        },
        baseline,
    })
}
async fn evidence(client: &CoreClient, id: &str, category: Category) -> InventoryEvidence {
    let target = DetailInput {
        id: Some(id.into()),
        name: None,
    };
    let input = crate::docker::ListInput {
        target,
        page: 0,
        page_size: 1,
    };
    let read = match category {
        Category::Image => crate::docker::images(client, input)
            .await
            .map(|v| (v.data.observed_count, v.data.scan_incomplete)),
        Category::Network => crate::docker::networks(client, input)
            .await
            .map(|v| (v.data.observed_count, v.data.scan_incomplete)),
        Category::Volume => crate::docker::volumes(client, input)
            .await
            .map(|v| (v.data.observed_count, v.data.scan_incomplete)),
        Category::Container => client
            .read(ListContainers { server: id.into() })
            .await
            .map(|v| (v.len(), v.len() > 512)),
        Category::System => unreachable!("system evidence is split into categories"),
    };
    match read {
        Ok((count, scan_incomplete)) => InventoryEvidence {
            category,
            cached_observed_count: Some(count.min(512)),
            scan_incomplete,
            error: None,
            evidence_limit: "Core cached observation only; empty/missing cache and suppressed poll failures cannot establish absence or a complete prune set. No generation timestamp, atomic dry run or current in-use proof.",
        },
        Err(error) => InventoryEvidence {
            category,
            cached_observed_count: None,
            scan_incomplete: true,
            error: Some(error),
            evidence_limit: "Supporting inventory unavailable. Target and Core Execute were independently resolved; no other target or category is substituted. The eventual prune set is not known.",
        },
    }
}
fn proof(p: &Prepared, category: Category) -> Value {
    json!({"server_id":p.server.id,"category":category,"baseline":p.baseline})
}
pub async fn preview(client: &CoreClient, input: PreviewInput) -> Result<PruneResult, AppError> {
    require(client)?;
    common::validate_payload(client, &input)?;
    let b = client.bounded_evidence();
    let p = prepare(&b, input.target.clone()).await?;
    let mut supporting_inventory = Vec::new();
    if input.category == Category::System {
        for category in [
            Category::Image,
            Category::Container,
            Category::Network,
            Category::Volume,
        ] {
            supporting_inventory.push(evidence(&b, &p.server.id, category).await);
        }
    } else {
        supporting_inventory.push(evidence(&b, &p.server.id, input.category).await);
    }
    let mut result = PruneResult {
        outcome: "preview",
        server: p.server.clone(),
        category: input.category,
        destructive_effect: input.category.effect(),
        core_operation: input.category.operation(),
        supporting_inventory,
        preview_id: None,
        prune_submissions: 0,
        receipt: None,
        exact_dry_run_available: false,
        evidence_limit: LIMITS,
    };
    let token = client.previews().insert_entry(Preview {
        created: Instant::now(),
        input: common::bound_input(OP, &input),
        baseline: p.baseline.clone(),
        prepared: proof(&p, input.category),
        result: result.clone(),
    })?;
    result.preview_id = Some(token);
    Ok(result)
}
pub(crate) async fn guard_execute(
    client: &CoreClient,
    operation: &'static str,
    params: &Value,
    private: &Value,
) -> Result<(), AppError> {
    require(client)?;
    let category: Category = serde_json::from_value(private["category"].clone())
        .map_err(|_| AppError::new(ErrorKind::InvalidPreview, OP, None))?;
    let id = private["server_id"]
        .as_str()
        .filter(|s| crate::deletion::object_id(s))
        .ok_or_else(|| AppError::new(ErrorKind::InvalidPreview, OP, None))?;
    if operation != category.operation() || *params != json!({"server":id}) {
        return Err(AppError::new(ErrorKind::InvalidPreview, OP, None));
    }
    let p = prepare(
        &client.bounded_evidence(),
        DetailInput {
            id: Some(id.into()),
            name: None,
        },
    )
    .await?;
    if proof(&p, category) != *private {
        return Err(stale());
    }
    Ok(())
}
pub async fn apply(client: &CoreClient, input: ApplyInput) -> Result<PruneResult, AppError> {
    require(client)?;
    if !input.acknowledge_destructive_effects {
        return Err(AppError::arguments(
            OP,
            "Explicitly acknowledge the previewed category's destructive effects and changing object set before applying.",
        ));
    }
    let bound_input = PreviewInput {
        target: input.target,
        category: input.category,
    };
    common::validate_payload(client, &bound_input)?;
    let entry = client
        .previews()
        .take_for::<PruneResult>(&input.preview_id, &common::bound_input(OP, &bound_input))?;
    let p = prepare(&client.bounded_evidence(), bound_input.target).await?;
    if p.baseline != entry.baseline || proof(&p, input.category) != entry.prepared {
        return Err(stale());
    }
    macro_rules! submit {
        ($ty:ident) => {
            client.prune_checked(
                $ty {
                    server: p.server.id.clone(),
                },
                &entry.prepared,
            )
        };
    }
    let effects = ConfiguredEffects {
        pre_deploy_hook: false,
        post_deploy_hook: false,
        auto_pull: None,
        saved_extra_args: false,
        saved_command_wrapper: false,
        broader_effects_possible: true,
        compose_dependencies: None,
        repository: None,
        build: None,
        procedure: None,
        action: None,
    };
    let resource = OperationResource {
        kind: WorkloadKind::Server,
        id: p.server.id.clone(),
        name: p.server.name.clone(),
    };
    let scope = RequestedScope::DockerPrune {
        category: input.category,
    };
    macro_rules! receipt {
        ($ty:ident) => {
            operations::submit_future(
                client,
                resource,
                scope,
                WorkloadOperation::Prune,
                input.category.core(),
                effects,
                submit!($ty),
            )
            .await?
        };
    }
    let receipt = match input.category {
        Category::Image => receipt!(PruneImages),
        Category::Container => receipt!(PruneContainers),
        Category::Network => receipt!(PruneNetworks),
        Category::Volume => receipt!(PruneVolumes),
        Category::System => receipt!(PruneSystem),
    };
    let mut result = entry.result;
    result.preview_id = None;
    result.outcome = "submitted";
    result.prune_submissions = 1;
    result.receipt = Some(receipt);
    Ok(result)
}

pub(crate) fn private_history(operation: Operation) -> bool {
    matches!(
        operation,
        Operation::PruneImages
            | Operation::PruneContainers
            | Operation::PruneNetworks
            | Operation::PruneVolumes
            | Operation::PruneSystem
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capabilities::CapabilityPolicy, client::RequestPolicy, config::Config};
    fn client(operate: bool, edit: bool, pruning: bool) -> CoreClient {
        CoreClient::new(
            Config::parse("http://127.0.0.1:1", "fixture-key", "fixture-secret")
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, edit).with_pruning(pruning)),
            RequestPolicy::default(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn closed_shared_transport_denies_disabled_or_invalid_reviews_without_network() {
        for operate in [false, true] {
            for edit in [false, true] {
                for pruning in [false, true] {
                    let c = client(operate, edit, pruning);
                    let error = c
                        .prune_checked(
                            PruneImages {
                                server: "000000000000000000000001".into(),
                            },
                            &Value::Null,
                        )
                        .await
                        .unwrap_err();
                    assert_eq!(
                        error.kind,
                        if operate && pruning {
                            ErrorKind::InvalidPreview
                        } else {
                            ErrorKind::CapabilityDisabled
                        }
                    );
                    assert_eq!(
                        c.execute(PruneImages {
                            server: "000000000000000000000001".into()
                        })
                        .await
                        .unwrap_err()
                        .kind,
                        ErrorKind::UnsupportedTarget
                    );
                    assert_eq!(
                        c.prune_checked(
                            PruneDockerBuilders {
                                server: "000000000000000000000001".into()
                            },
                            &Value::Null
                        )
                        .await
                        .unwrap_err()
                        .kind,
                        ErrorKind::UnsupportedTarget
                    );
                    assert_eq!(
                        c.prune_checked(
                            PruneBuildx {
                                server: "000000000000000000000001".into()
                            },
                            &Value::Null
                        )
                        .await
                        .unwrap_err()
                        .kind,
                        ErrorKind::UnsupportedTarget
                    );
                }
            }
        }
        let c = client(true, false, true);
        let private =
            json!({"server_id":"000000000000000000000001","category":"system","baseline":{}});
        assert_eq!(
            guard_execute(
                &c,
                "PruneImages",
                &json!({"server":"000000000000000000000001"}),
                &private
            )
            .await
            .unwrap_err()
            .kind,
            ErrorKind::InvalidPreview
        );
        assert_eq!(
            guard_execute(
                &c,
                "PruneSystem",
                &json!({"server":"000000000000000000000001","flags":["--filter=x"]}),
                &private
            )
            .await
            .unwrap_err()
            .kind,
            ErrorKind::InvalidPreview
        );
    }
    #[test]
    fn strict_pruning_configuration_preserves_legacy_defaults_and_independent_grants() {
        for operate in [false, true] {
            for edit in [false, true] {
                let legacy = CapabilityPolicy::new(operate, edit);
                assert!(!legacy.pruning_enabled());
                assert!(!legacy.allows(Capability::Prune));
                for pruning in [false, true] {
                    let policy = legacy.with_pruning(pruning);
                    assert_eq!(policy.allows(Capability::Prune), operate && pruning);
                    assert_eq!(policy.allows(Capability::ConfigurationEdit), edit);
                    assert_eq!(policy.allows(Capability::Operate), operate);
                }
            }
        }
        for value in [None, Some("false"), Some("true")] {
            assert_eq!(
                crate::capabilities::parse_grant("KOMODO_ALLOW_PRUNE", value).unwrap(),
                value == Some("true")
            );
        }
        for bad in ["", "TRUE", "1", " true", "private-flag-marker"] {
            let error =
                crate::capabilities::parse_grant("KOMODO_ALLOW_PRUNE", Some(bad)).unwrap_err();
            assert_eq!(error.kind, ErrorKind::Configuration);
            assert_eq!(error.field, Some("KOMODO_ALLOW_PRUNE"));
            assert!(!format!("{error:?}").contains("private-flag-marker"));
        }
    }
}
