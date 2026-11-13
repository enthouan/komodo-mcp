use rmcp::{
    RoleServer, ServerHandler,
    handler::server::tool::schema_for_input,
    model::{CallToolResult, Implementation, JsonObject, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_handler, tool_router,
};

use crate::{
    capabilities::{Capability, restrict_router},
    client::{CoreClient, CoreVersion},
    inventory::{self, *},
};

pub struct KomodoServer {
    pub client: CoreClient,
    pub core_version: CoreVersion,
}

impl KomodoServer {
    fn enabled_tools(&self) -> rmcp::handler::server::tool::ToolRouter<Self> {
        restrict_router(
            Self::tool_router(),
            self.client.capabilities(),
            |name| match name {
                "list_servers"
                | "get_server"
                | "list_stacks"
                | "get_stack"
                | "list_deployments"
                | "get_deployment"
                | "fleet_health"
                | "list_alerts"
                | "list_updates"
                | "get_update"
                | "wait_for_update"
                | "reconcile_operation"
                | "get_container_logs"
                | "get_deployment_logs"
                | "get_stack_logs"
                | "stack_diagnose" => Some(Capability::Read),
                "deploy_stack" | "deploy_deployment" | "start_stack" | "stop_stack"
                | "restart_stack" | "start_deployment" | "stop_deployment"
                | "restart_deployment" => Some(Capability::Operate),
                "apply_resource_delete" => Some(Capability::Destructive),
                "preview_stack_update"
                | "apply_stack_update"
                | "preview_stack_create"
                | "apply_stack_create"
                | "preview_deployment_create"
                | "apply_deployment_create"
                | "preview_deployment_update"
                | "apply_deployment_update"
                | "preview_server_create"
                | "apply_server_create"
                | "preview_server_update"
                | "apply_server_update"
                | "preview_resource_delete" => Some(Capability::ConfigurationEdit),
                _ => None,
            },
        )
    }
}

#[tool_router]
impl KomodoServer {
    #[tool(description = "Preview deliberate deletion of exactly one Stack, Deployment or Server by kind and stable ID, with action=delete and trusted configuration access. Resolve through positive inventory policy, identify deployed cleanup names, fresh known container IDs and bounded visible Alerter/Server-dependent references; disclose hidden, truncated and unavailable effects. Core can tear down live workloads inside a definition write, skip cleanup or remove the record despite cleanup errors. Server deletion leaves host workloads and can leave current Builder server_ids references dangling. Return the shared bounded ten-minute single-use preview_id. Applying requires BOTH local grants regardless of previewed state. No mutation is submitted by preview.", input_schema = schema_for_input::<crate::deletion::PreviewInput>().expect("deletion preview schema"), output_schema = inventory::output_schema::<crate::deletion::DeletionResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = true))]
    async fn preview_resource_delete(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_resource_delete") {
            Ok(input) => crate::deletion::preview(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Deliberately apply the exact resource kind/stable ID, action=delete and preview_id once. BOTH configuration-edit and operate must be enabled; discovery and direct dispatch are disabled without either. Revalidate raw definition, deployed cleanup identity, attachment, relevant runtime, history and visible reference effects before one typed delete. Core requires Write and can remove containers/services/stacks before deleting the definition. No bulk selection, separate MCP execution, provisioning, retry or credential fallback. Keep acknowledgement, direct definition observation, fresh known-container inspection, visible reference after-state and cleanup-update candidates/errors separate. Core can hide history after removing resource permissions; successful update flags or missing cache data do not prove cleanup. Core archives secret-bearing Deleted Toml; MCP never returns that stdout. Final-read/write races remain. Lost/invalid/oversized/mismatched responses retain submission_unknown; never automatically repeat deletion.", input_schema = schema_for_input::<crate::deletion::ApplyInput>().expect("deletion apply schema"), output_schema = inventory::output_schema::<crate::deletion::DeletionResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_resource_delete(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_resource_delete") {
            Ok(input) => crate::deletion::apply(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview creation of one Server record with trusted configuration access and zero writes. Require a name and explicit address (empty selects inbound), enabled and insecure_tls settings. Optional initial_public_key establishes initial pinned trust only; enabled creation requires a key and applying it requires both local grants. New records explicitly disable automatic image pruning and automatic key rotation. Return bounded redacted changes and a shared ten-minute single-use preview_id. Core enforces admin or create_server_permissions plus non-admin creation policy. This does not provision a host, run a shell, create a network or deploy a workload.", input_schema = schema_for_input::<crate::server_configuration::CreateInput>().expect("server create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_server_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_server_create") {
            Ok(input) => crate::server_configuration::preview_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact Server creation proposal and preview_id once with trusted configuration access; enabled creation also requires operate. Recheck bounded visible name absence and submit one typed CreateServer with explicit initial trust only. Verify the echoed saved config and key privately. Observe Core-reported connectivity separately after saving: a saved record can be unreachable, and even cached Ok does not verify a new connection. Core may reconnect and poll system/Docker/Repo state. No provisioning, shell, container lifecycle or key-rotation request is submitted. Uncertain replies consume the preview and are never retried.", input_schema = schema_for_input::<crate::server_configuration::CreateApplyInput>().expect("server create apply schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_server_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_server_create") {
            Ok(input) => crate::server_configuration::apply_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview a typed partial Server connection, region, stats monitoring, alert-toggle or ignore_mounts edit with trusted configuration access and zero writes. Omission preserves values; null is invalid; empty address selects inbound, empty region and ignore_mounts clear. Raw config and pinned public key stay private in the shared stale baseline. Address, enabled or insecure_tls applies require both local grants independently of current policies. Enabled writes require existing pinned trust. No public-key replacement, changed-key acceptance, rotation, deprecated passkey edit or provisioning fallback is supported. Desired config and preview-time cached connectivity remain separate.", input_schema = schema_for_input::<crate::server_configuration::PreviewInput>().expect("server update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_server_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_server_update") {
            Ok(input) => crate::server_configuration::preview_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact Server target/patch and shared preview_id once. Configuration-edit is required; any supplied address/enabled/insecure_tls also requires operate before Core reads, because reconnection and existing background prune/key policies can follow the target. Revalidate raw config and pinned key; stale/no-op writes are zero. Submit only a typed partial UpdateServer preserving omitted settings and trust. Core refreshes connectivity independently of the save; report cached connectivity or a sanitized observation error separately. No host provisioning, shell, lifecycle, public-key update or rotation request, credential fallback or automatic retry. Final-read/write races remain without Core conditional writes.", input_schema = schema_for_input::<crate::server_configuration::ApplyInput>().expect("server apply schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_server_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_server_update") {
            Ok(input) => crate::server_configuration::apply_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview creation of one desired Deployment definition with trusted configuration access. Require a name, explicit image reference or resolved Build plus major.minor.patch version (0.0.0 selects latest at deploy), and an existing Server/Swarm attachment. Optional settings preserve Core defaults. Return normalized name, redacted changes, source/target and zero writes with the shared ten-minute single-use preview_id. Core enforces creation policy and Read/Attach permissions. Saving is separate from runtime state and issues no MCP deploy, restart, image pull or shell request.", input_schema = schema_for_input::<crate::deployment_configuration::CreateInput>().expect("deployment create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_deployment_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_deployment_create") {
            Ok(input) => crate::deployment_configuration::preview_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact Deployment creation input and shared preview_id once with trusted configuration access. Revalidate visible name absence and raw Server/Swarm/Build references, then submit one typed CreateDeployment. Core owns global uniqueness and creation/Read-Attach permissions; hidden names and concurrent creators remain possible. No deployment or image pull is submitted by MCP and no auto-update policy is enabled. A saved definition does not establish running configuration or health. Lost/invalid replies remain submission_unknown; never automatically retry or replay consumed previews.", input_schema = schema_for_input::<crate::deployment_configuration::CreateApplyInput>().expect("deployment create apply schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_deployment_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_deployment_create") {
            Ok(input) => crate::deployment_configuration::apply_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview a typed partial desired Deployment edit with trusted configuration access, zero writes and the shared single-use preview contract. Support image/Build reference, attachment, registry-account reference, environment, ports, volumes, command, network, extra_args, labels, send_alerts and replicas. Omission preserves fields; null and display redaction markers are rejected; documented empty strings/lists explicitly clear. Applying any image requires BOTH operate and configuration-edit grants, regardless of current auto-update/build policy, because Core can auto-deploy. Keep desired saved image/configuration separate from preview-time runtime state: Core's reported image may fall back to desired configuration and does not prove the running image. Raw configuration/references remain private and bound to this proposal.", input_schema = schema_for_input::<crate::deployment_configuration::PreviewInput>().expect("deployment preview schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_deployment_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_deployment_update") {
            Ok(input) => crate::deployment_configuration::preview_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact desired Deployment target/patch and shared preview_id once with trusted configuration access. Any supplied image also requires BOTH operate and configuration-edit grants before Core reads, independently of the observed auto_update policy. Revalidate raw baseline and references, then submit only the typed partial UpdateDeployment; no-op and stale proposals submit zero writes. MCP submits no deployment/restart/pull/shell request. Direct image writes trigger Core registry login/manifest inspection and can auto-deploy under Core policy; Build references can follow existing build-triggered redeploy policy. Inspected Core 2.3.2 target reassignment leaves old workloads in place despite the SDK warning. Custom-name mutation is unsupported and existing names are preserved. Lost/invalid replies retain uncertainty, with no retries; Core final-read/write races remain. Saved desired configuration is separate from runtime verification.", input_schema = schema_for_input::<crate::deployment_configuration::ApplyInput>().expect("deployment apply schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_deployment_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_deployment_update") {
            Ok(input) => crate::deployment_configuration::apply_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview creation of one Stack definition with trusted configuration access. Supply a name, explicit inline/Git/linked-Repo/host source and an existing Server/Swarm attachment; optional settings omit default fields. Returns Core's normalized name, source/target and redacted changes with zero writes. Existing host/Git file contents are not edited. Core still enforces creation and Read/Attach permissions; visible name absence cannot prove global absence. Reuse the exact input and returned process-local single-use preview_id with apply_stack_create. Creation saves desired configuration without a deployment command or a runtime health claim.", input_schema = schema_for_input::<crate::configuration::StackCreateInput>().expect("create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_stack_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_stack_create") {
            Ok(input) => crate::configuration::preview_stack_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact Stack creation input and preview_id once with trusted configuration access. Recheck visible name absence and raw referenced definitions, then submit one typed CreateStack. Core owns global uniqueness and creation/Read-Attach authorization; no credentials are changed. A save is separate from deployment and health. No host-file write, Git commit or execution request is submitted. Lost/invalid replies retain submission_unknown; never automatically retry creation. Consumed/expired/restarted-process previews cannot be replayed, and Core has no upstream idempotency or conditional-create parameter.", input_schema = schema_for_input::<crate::configuration::StackCreateApplyInput>().expect("create apply schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_stack_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_stack_create") {
            Ok(input) => crate::configuration::apply_stack_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview a typed partial Stack edit with trusted configuration access. Reads authoritative configuration and returns redacted changed fields, resolved identity and a ten-minute process-local single-use preview_id; performs zero writes, including no-op edits. Edit auto_pull, environment, ignore_services, project_name, source or attachment. Applying any supplied source also requires the independent operate grant, even if auto_update is currently disabled, because Core policy can race; configuration-only callers may preview it. Omitted fields are preserved; null is rejected; documented empty values clear supported fields. Source selectors distinguish inline, Git, linked Repo and existing host files; attachment resolves an existing Server or Swarm. Never copy display redaction markers into input. Saving is separate from deployment and health. MCP submits no deploy/file-write/Git-commit command. Core refreshes caches and source-field updates trigger its update check: an enabled saved auto_update policy can deploy through Core, so source apply requires both local grants. Inspect the previewed effects. Apply must use the exact same target and patch; the authoritative baseline is kept privately, including secrets.", input_schema = schema_for_input::<crate::configuration::StackPreviewInput>().expect("valid preview schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_stack_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_stack_update") {
            Ok(input) => crate::configuration::preview_stack_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact Stack target/patch from preview_stack_update once with trusted configuration access. Consume the preview before I/O, recheck authoritative configuration, and submit only supplied typed fields; no-op and stale previews submit zero writes. Core has no conditional write: another writer can race after the final read. Never automatically retry after timeout, cancellation, lost or invalid replies; submission_unknown retains uncertainty and the token cannot be replayed. Prepare a fresh preview only after inspecting saved configuration. Applying a supplied source requires BOTH operate and configuration-edit grants, checked locally before any Core reads regardless of the observed auto_update policy. MCP submits no deployment or lifecycle command; source changes can trigger Core auto-deploy when the previewed auto_update policy is enabled. The inspected Core 2.3.2 target-reassignment path refreshes the new target and leaves old workloads in place despite the SDK cleanup warning. Inspect both targets. Stored configuration does not establish runtime health.", input_schema = schema_for_input::<crate::configuration::StackApplyInput>().expect("valid apply schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_stack_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_stack_update") {
            Ok(input) => crate::configuration::apply_stack_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect bounded recent history after a lost execution reply when no update ID is known. Reads only, no operate grant needed. Supply the original resolved target, requested whole/service scope, operation and available local UTC submission-window bounds. Search at most three fixed 100-record history pages and five service-detail lookups, with two-second read and ten-second total budgets; return at most five possible candidates. Core service logs may be absent, inaccessible or ambiguous. Zero candidates never proves non-acceptance, and one candidate never proves attribution: another caller can issue an identical action. Original submission remains unknown; no candidate is selected automatically and execution is never resubmitted. Retry observation for delayed history, not execution. If an acknowledged update ID is known, use get_update/wait_for_update directly. Candidate execution outcome is separate from workload health.", input_schema = schema_for_input::<crate::recovery::RecoveryInput>().expect("valid recovery schema"), output_schema = inventory::output_schema::<crate::recovery::RecoveryResult>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn reconcile_operation(
        &self,
        arguments: JsonObject,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let result = match inventory::parse(arguments, "reconcile_operation") {
            Ok(input) => {
                crate::recovery::reconcile_operation(&self.client, input, context.ct.cancelled())
                    .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Observe one known Core update ID with reads only; available without operate. Wait up to 1..30 seconds (default 10), at most 10 lookups, at least one second after each unfinished response. Return the ID, latest bounded observation and explicit end reason. A local deadline, request cap, read failure or cancelled wait is incomplete observation, not remote operation failure/cancellation. Resume with this tool or get_update in a new session using the same saved ID; never execute again to resume. Core Complete plus its success field determines final outcome; it does not establish workload health. Use fleet_health/stack_diagnose/logs separately. MCP cancellation stops local polling; save the ID before waiting because cancelled requests may have no response.", input_schema = schema_for_input::<crate::wait::WaitInput>().expect("valid wait schema"), output_schema = inventory::output_schema::<crate::wait::WaitResult>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn wait_for_update(
        &self,
        arguments: JsonObject,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        let result = match inventory::parse(arguments, "wait_for_update") {
            Ok(input) => {
                crate::wait::wait_for_update(&self.client, input, context.ct.cancelled()).await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Deploy one existing Stack from its saved definition and image-pull policy. Explicitly choose whole_stack or one exact Compose service; dependencies, saved hooks, flags and wrappers can have broader effects. Swarm supports whole-stack deploy only. Unlike restart, deploy can recreate containers with saved changes. Submit once, return an update receipt and one immediate bounded observation; acknowledgement is not rollout success. Never automatically retry execution after a timeout/lost reply: use get_update/wait_for_update for a known ID or reconcile_operation for missing-ID candidates. A new explicit call is a new non-idempotent operation.", input_schema = schema_for_input::<crate::operations::StackOperationInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn deploy_stack(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "deploy_stack") {
            Ok(input) => crate::operations::deploy_stack(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Deploy one existing Deployment by stable ID or exact name, using saved configuration/image selection and Core's deploy image-pull behavior. Supports Server containers and Swarm services; no service filter or overrides. Deploy can recreate a container; restart uses existing containers. Submit at most once and return an update receipt with one immediate bounded observation. Acceptance does not establish execution/rollout success or workload health. Never automatically retry a lost/uncertain submission; use get_update/wait_for_update for a known ID or reconcile_operation for missing-ID candidates. A new explicit call can repeat effects.", input_schema = schema_for_input::<DetailInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn deploy_deployment(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "deploy_deployment") {
            Ok(input) => crate::operations::deploy_deployment(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Start one existing Server-backed Stack by stable ID or exact name. Swarm is rejected before execution. Choose whole_stack or one exact deployed Compose service. Compose can start existing dependencies. Uses existing containers; does not deploy saved changes, pull images or run deploy hooks. Paused/unavailable containers retain Core's actual result; never substitute unpause or redeploy. Submit at most once and return an update receipt with one bounded observation. Acceptance is not execution success or workload health. Never automatically retry after a timeout/lost reply; use get_update/wait_for_update for a known ID or reconcile_operation for missing-ID candidates. A new explicit call can repeat effects.", input_schema = schema_for_input::<crate::operations::StackOperationInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn start_stack(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "start_stack") {
            Ok(input) => {
                crate::operations::stack_operation(
                    &self.client,
                    input,
                    crate::operations::WorkloadOperation::Start,
                )
                .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Start one existing Server-backed Deployment by stable ID or exact name. Swarm is rejected before execution. Uses existing containers; does not deploy saved changes, pull images or run deploy hooks. Paused/unavailable containers retain Core's actual result; never substitute unpause or redeploy. Submit at most once and return an update receipt with one bounded observation. Acceptance is not execution success or workload health. Never automatically retry after a timeout/lost reply; use get_update/wait_for_update for a known ID or reconcile_operation for missing-ID candidates. A new explicit call can repeat effects.", input_schema = schema_for_input::<DetailInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn start_deployment(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "start_deployment") {
            Ok(input) => {
                crate::operations::deployment_operation(
                    &self.client,
                    input,
                    crate::operations::WorkloadOperation::Start,
                )
                .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Stop one existing Server-backed Stack by stable ID or exact name. Swarm is rejected before execution. Choose whole_stack or one exact deployed Compose service. Compose stops selected services without removing them; no dependency expansion is requested. Uses existing containers; does not deploy saved changes, pull images or run deploy hooks. Paused/unavailable containers retain Core's actual result; never substitute unpause or redeploy. Submit at most once and return an update receipt with one bounded observation. Acceptance is not execution success or workload health. Never automatically retry after a timeout/lost reply; use get_update/wait_for_update for a known ID or reconcile_operation for missing-ID candidates. A new explicit call can repeat effects.", input_schema = schema_for_input::<crate::operations::StackOperationInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn stop_stack(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "stop_stack") {
            Ok(input) => {
                crate::operations::stack_operation(
                    &self.client,
                    input,
                    crate::operations::WorkloadOperation::Stop,
                )
                .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Stop one existing Server-backed Deployment by stable ID or exact name. Swarm is rejected before execution. Uses existing containers; does not deploy saved changes, pull images or run deploy hooks. Paused/unavailable containers retain Core's actual result; never substitute unpause or redeploy. Submit at most once and return an update receipt with one bounded observation. Acceptance is not execution success or workload health. Never automatically retry after a timeout/lost reply; use get_update/wait_for_update for a known ID or reconcile_operation for missing-ID candidates. A new explicit call can repeat effects.", input_schema = schema_for_input::<DetailInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn stop_deployment(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "stop_deployment") {
            Ok(input) => {
                crate::operations::deployment_operation(
                    &self.client,
                    input,
                    crate::operations::WorkloadOperation::Stop,
                )
                .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Restart one existing Server-backed Stack by stable ID or exact name. Swarm is rejected before execution. Choose whole_stack or one exact deployed Compose service. Compose may restart dependents configured with depends_on.restart; Core adds no --no-deps. Uses existing containers; does not deploy saved changes, pull images or run deploy hooks. Paused/unavailable containers retain Core's actual result; never substitute unpause or redeploy. Submit at most once and return an update receipt with one bounded observation. Acceptance is not execution success or workload health. Never automatically retry after a timeout/lost reply; use get_update/wait_for_update for a known ID or reconcile_operation for missing-ID candidates. A new explicit call can repeat effects.", input_schema = schema_for_input::<crate::operations::StackOperationInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn restart_stack(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "restart_stack") {
            Ok(input) => {
                crate::operations::stack_operation(
                    &self.client,
                    input,
                    crate::operations::WorkloadOperation::Restart,
                )
                .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Restart one existing Server-backed Deployment by stable ID or exact name. Swarm is rejected before execution. Uses existing containers; does not deploy saved changes, pull images or run deploy hooks. Paused/unavailable containers retain Core's actual result; never substitute unpause or redeploy. Submit at most once and return an update receipt with one bounded observation. Acceptance is not execution success or workload health. Never automatically retry after a timeout/lost reply; use get_update/wait_for_update for a known ID or reconcile_operation for missing-ID candidates. A new explicit call can repeat effects.", input_schema = schema_for_input::<DetailInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn restart_deployment(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "restart_deployment") {
            Ok(input) => {
                crate::operations::deployment_operation(
                    &self.client,
                    input,
                    crate::operations::WorkloadOperation::Restart,
                )
                .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "List Servers in ascending exact-name order with bounded inventory pages. Continue with next_page and the same page_size.", input_schema = schema_for_input::<ListInput>().expect("valid list schema"), output_schema = inventory::output_schema::<Page<ServerSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_servers(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "ListServers") {
            Ok(input) => inventory::list_servers(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect one Server by exactly one stable id or exact case-sensitive name. Names are never interpreted as IDs; ambiguous names require an explicit ID.", input_schema = schema_for_input::<DetailInput>().expect("valid selector schema"), output_schema = inventory::output_schema::<ServerDetail>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_server(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "GetServer") {
            Ok(input) => inventory::get_server(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "List Stacks in ascending exact-name order with bounded inventory pages. Continue with next_page and the same page_size.", input_schema = schema_for_input::<ListInput>().expect("valid list schema"), output_schema = inventory::output_schema::<Page<StackSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_stacks(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "ListStacks") {
            Ok(input) => inventory::list_stacks(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect one Stack by exactly one stable id or exact case-sensitive name. Names are never interpreted as IDs; ambiguous names require an explicit ID. A Swarm target takes precedence over a Server.", input_schema = schema_for_input::<DetailInput>().expect("valid selector schema"), output_schema = inventory::output_schema::<StackDetail>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_stack(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "GetStack") {
            Ok(input) => inventory::get_stack(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "List Deployments in ascending exact-name order with bounded inventory pages. Continue with next_page and the same page_size.", input_schema = schema_for_input::<ListInput>().expect("valid list schema"), output_schema = inventory::output_schema::<Page<DeploymentSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_deployments(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "ListDeployments") {
            Ok(input) => inventory::list_deployments(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect one Deployment by exactly one stable id or exact case-sensitive name. Names are never interpreted as IDs; ambiguous names require an explicit ID. A Swarm target takes precedence over a Server.", input_schema = schema_for_input::<DetailInput>().expect("valid selector schema"), output_schema = inventory::output_schema::<DeploymentDetail>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_deployment(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "GetDeployment") {
            Ok(input) => inventory::get_deployment(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect one bounded page of fleet health, utilization and workload services. Keep partial results and resource-specific gaps when a source fails; continue with next_page.", input_schema = schema_for_input::<crate::health::FleetHealthInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::health::FleetHealth>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn fleet_health(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "fleet_health") {
            Ok(input) => crate::health::fleet_health(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Read one fixed Core page of at most 100 recent alerts, optionally filtered by resource kind and stable ID. Follow next_page with the same target.", input_schema = schema_for_input::<crate::history::HistoryInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::history::HistoryPage<crate::history::AlertSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_alerts(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_alerts") {
            Ok(input) => crate::history::list_alerts(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Read one fixed Core page of at most 100 recent updates, optionally filtered by resource. Queued, running, succeeded and failed outcomes are distinct.", input_schema = schema_for_input::<crate::history::HistoryInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::history::HistoryPage<crate::history::UpdateSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_updates(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_updates") {
            Ok(input) => crate::history::list_updates(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect an update by stable ID with explicit outcome and at most 20 bounded stages, failures first. Raw configuration, command, stdout and other_data are omitted.", input_schema = schema_for_input::<crate::history::UpdateInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::history::UpdateDetail>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_update") {
            Ok(input) => crate::history::get_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Read a bounded container log tail by stable Server ID and exact container name. Tail 1..500; combined stdout/stderr max_bytes 1..65536 after redaction, with truncation metadata.", input_schema = schema_for_input::<crate::logs::ContainerLogsInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::logs::WorkloadLogs>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_container_logs(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_container_logs") {
            Ok(input) => crate::logs::container_logs(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Read a bounded Deployment log tail by stable ID. Tail 1..500 and max_bytes 1..65536 apply jointly to redacted stdout/stderr; no unbounded fallback.", input_schema = schema_for_input::<crate::logs::DeploymentLogsInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::logs::WorkloadLogs>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_deployment_logs(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_deployment_logs") {
            Ok(input) => crate::logs::deployment_logs(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Read a bounded Stack log tail by stable ID and exact service filters (Compose: optional; Swarm: pass one). Tail 1..500 and max_bytes 1..65536 apply jointly after redaction; includes truncation metadata.", input_schema = schema_for_input::<crate::logs::StackLogsInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::logs::WorkloadLogs>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_stack_logs(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_stack_logs") {
            Ok(input) => crate::logs::stack_logs(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Diagnose a stack by stable ID in one read-only call. Correlate service health, updates, alerts and bounded filtered logs; report healthy, unhealthy or inconclusive and retain evidence with source gaps.", input_schema = schema_for_input::<crate::triage::StackDiagnoseInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::triage::StackDiagnosis>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn stack_diagnose(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "stack_diagnose") {
            Ok(input) => crate::triage::stack_diagnose(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }
}

#[tool_handler(router = self.enabled_tools())]
impl ServerHandler for KomodoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        )
    }
}
