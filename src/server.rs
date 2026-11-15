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
                | "export_resources_toml"
                | "get_server"
                | "list_stacks"
                | "get_stack"
                | "list_deployments"
                | "get_deployment"
                | "list_repos"
                | "get_repo"
                | "list_resource_syncs"
                | "get_resource_sync"
                | "get_resource_sync_pending"
                | "review_resource_sync_pending"
                | "wait_for_resource_sync_update"
                | "list_builders"
                | "get_builder"
                | "list_builds"
                | "get_build"
                | "list_procedures"
                | "get_procedure"
                | "list_actions"
                | "get_action"
                | "list_schedules"
                | "get_schedule"
                | "fleet_health"
                | "list_alerts"
                | "list_updates"
                | "get_update"
                | "wait_for_update"
                | "reconcile_operation"
                | "get_container_logs"
                | "get_deployment_logs"
                | "get_stack_logs"
                | "stack_diagnose"
                | "find_resources_by_tag" => Some(Capability::Read),
                "deploy_stack"
                | "deploy_deployment"
                | "start_stack"
                | "stop_stack"
                | "restart_stack"
                | "start_deployment"
                | "stop_deployment"
                | "restart_deployment"
                | "clone_repo"
                | "pull_repo"
                | "run_build"
                | "cancel_build"
                | "run_procedure"
                | "cancel_procedure"
                | "run_action"
                | "cancel_action"
                | "refresh_resource_sync_pending" => Some(Capability::Operate),
                "apply_resource_delete" | "apply_tag_delete" | "apply_resource_sync" => {
                    Some(Capability::Destructive)
                }
                "preview_stack_update"
                | "apply_stack_update"
                | "preview_repo_create"
                | "preview_resource_sync_create"
                | "apply_resource_sync_create"
                | "preview_resource_sync_update"
                | "apply_resource_sync_update"
                | "apply_repo_create"
                | "preview_repo_update"
                | "apply_repo_update"
                | "preview_builder_create"
                | "apply_builder_create"
                | "preview_builder_update"
                | "apply_builder_update"
                | "preview_build_create"
                | "apply_build_create"
                | "preview_build_update"
                | "apply_build_update"
                | "preview_schedule_update"
                | "apply_schedule_update"
                | "preview_action_create"
                | "apply_action_create"
                | "preview_action_update"
                | "apply_action_update"
                | "preview_procedure_create"
                | "apply_procedure_create"
                | "preview_procedure_update"
                | "apply_procedure_update"
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
                | "preview_resource_delete"
                | "preview_tag_create"
                | "apply_tag_create"
                | "preview_tag_rename"
                | "apply_tag_rename"
                | "preview_resource_tags"
                | "apply_resource_tags"
                | "preview_tag_delete" => Some(Capability::ConfigurationEdit),
                _ => None,
            },
        )
    }
}

#[tool_router]
impl KomodoServer {
    #[tool(description="Apply exactly one reviewed ResourceSync and exact apply_binding. Requires BOTH configuration-edit and operate before Core I/O; Core Execute remains authoritative. Consume the ten-minute process-local review once, privately recheck definition/dependencies/full plan/source, submit one explicit pending-cache refresh, reject any drift/errors/incomplete evidence, then submit at most one RunSync. Refresh can fetch source/cache and affect alerts; RunSync can internally refresh and act through Core's built-in administrator sync identity. Whole/type/named scopes stay explicit; filtered runs exclude variables/groups. Managed OR delete permits deletion; empty tags are unrestricted. Core rereads sources without expected revision: a final-read race remains, with no frozen plan, exactly-once guarantee, rollback or remote cancellation. Return separate refresh/RunSync counts, accepted/rejected/unknown receipt, known update ID/window, bounded follow-up, safe batch reports and independent resource observations. Parent success does not verify child deployment success or workload health. No replay or authoritative Git/host write-back.",input_schema=schema_for_input::<crate::sync_plans::ApplyInput>().expect("apply sync schema"),output_schema=inventory::output_schema::<crate::sync_plans::ApplyResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn apply_resource_sync(
        &self,
        context: RequestContext<RoleServer>,
        arguments: JsonObject,
    ) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_resource_sync") {
            Ok(input) => {
                crate::sync_plans::apply(&self.client, input, context.ct.cancelled()).await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Resume one explicitly known RunSync update ID with the retained sync/scope binding, including after MCP restart. Read-only; no refresh, execute, replay or remote cancellation. Validate stable sync ID and RunSync operation on EVERY poll, preserve matching prior evidence on mismatch/read failure, and stop within 1–30 seconds or ten polls. Core history does not record reliable selectors or invocation ownership; retained selection is an expectation, never inferred attribution. Unknown-ID recovery candidates from reconcile_operation remain unconfirmed. Parent completion/batch success does not verify child deployments or workload health; inspect resources and health independently. Raw sync output/errors/TOML are withheld.",input_schema=schema_for_input::<crate::sync_plans::FollowInput>().expect("sync follow schema"),output_schema=inventory::output_schema::<crate::sync_plans::FollowResult>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn wait_for_resource_sync_update(
        &self,
        context: RequestContext<RoleServer>,
        arguments: JsonObject,
    ) -> CallToolResult {
        let result = match inventory::parse(arguments, "wait_for_resource_sync_update") {
            Ok(input) => {
                crate::sync_plans::follow(&self.client, input, context.ct.cancelled()).await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Read one ResourceSync's cached pending creates, updates, deletions, variable/group changes, deployment intent and distinct pending/deployment/source-file errors. Return bounded sanitized summaries; never return raw TOML, source contents, variable values or error snippets. Distinguish cached observations and available Git revision from actual current source freshness; inline/host have no Git hash. This calls only Core read APIs and submits zero mutations. Core's cached view can omit template-only and variable-secret-flag effects; use explicit scoped review for apply coverage. Display truncation or a cached clean state never authorizes RunSync.",input_schema=schema_for_input::<DetailInput>().expect("cached pending schema"),output_schema=inventory::output_schema::<crate::sync_plans::PendingOutput>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn get_resource_sync_pending(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_resource_sync_pending") {
            Ok(input) => crate::sync_plans::get(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Explicitly refresh one resolved ResourceSync through exactly one typed RefreshResourceSyncPending. Requires the independent operate grant before reads and Core Execute; read-only credentials remain denied, with no fallback. Count the refresh attempt separately from zero RunSync requests. Core can read sources, fetch/clone its Git cache and create/resolve/send pending alerts. Return a bounded sanitized refresh-response observation, including distinct pending/deployment/file errors; unconfigured syncs can be skipped by Core. Lost or invalid acknowledgements preserve cache-mutation uncertainty without retry. No desired-state application or authoritative Git/host write-back.",input_schema=schema_for_input::<DetailInput>().expect("refresh pending schema"),output_schema=inventory::output_schema::<crate::sync_plans::RefreshOutput>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=false))]
    async fn refresh_resource_sync_pending(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "refresh_resource_sync_pending") {
            Ok(input) => crate::sync_plans::refresh(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Review an EXPLICIT whole-sync, resource-type or typed named-resource selection using only Core reads. Bind stable IDs for existing resources and exact pending-create names for new resources; invalid/missing/duplicate selectors never become an unfiltered run. Audit template-only and variable secret-flag effects missing from Core's cached view. Variables/groups are excluded from filtered scope. Reject incomplete/truncated/unfaithful coverage, pending errors and deployment scope combinations that differ in Core 2.3.2 execution. When complete, retain full authoritative config/plan/source/reference evidence privately in the shared bounded ten-minute single-use store, returning an opaque review_reference and exact apply_binding for the later separately authorized apply. Refuse review authority on unsupported Core versions. Redacted display is never the comparison baseline. This performs zero Core mutations or RunSync, and does not prove an external source current or freeze final-read/execution races.",input_schema=schema_for_input::<crate::sync_plans::ReviewInput>().expect("review pending schema"),output_schema=inventory::output_schema::<crate::sync_plans::ReviewOutput>(),annotations(read_only_hint=true,idempotent_hint=false,destructive_hint=false))]
    async fn review_resource_sync_pending(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "review_resource_sync_pending") {
            Ok(input) => crate::sync_plans::review(&self.client, &self.core_version, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "List visible ResourceSync definitions with bounded positive pagination and name-ascending order. Return sanitized identity/source kind, management and cached state only. Use the same page_size with next_page. This calls only Core read APIs; no refresh, RunSync or source-file write.", input_schema = schema_for_input::<ListInput>().expect("list_resource_syncs schema"), output_schema = inventory::output_schema::<Page<crate::resource_syncs::SyncSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_resource_syncs(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_resource_syncs") {
            Ok(input) => crate::resource_syncs::list(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect one exact ResourceSync by stable ID or exact name. Return sanitized source/settings, effective managed-or-delete policy, tag restriction, separate variable/group flags and webhook configuration; omit inline TOML, credentials, webhook secrets and pending raw content/errors. Core-host sources are under sync_directory / normalized sync name. Cached inspection calls no refresh or execution. Raw definitions remain private.", input_schema = schema_for_input::<DetailInput>().expect("get_resource_sync schema"), output_schema = inventory::output_schema::<crate::resource_syncs::SyncDetail>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_resource_sync(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_resource_sync") {
            Ok(input) => crate::resource_syncs::get(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview creating a ResourceSync with an explicit inline, direct-Git, linked-Repo or Core-host source. Creation requires an administrator in Core; Read/Attach on a linked Repo remains authoritative. Use bounded trusted inline desired-state TOML; omitted desired fields can reset defaults. External sources require explicit resource_paths on creation; [] reads no files. New webhooks default OFF explicitly. Preview managed deletion, filters, inclusion and webhook settings with zero mutations; bind private linked Repo state and exact input to a ten-minute single-use preview_id.", input_schema = schema_for_input::<crate::resource_syncs::CreateInput>().expect("preview_resource_sync_create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_resource_sync_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_resource_sync_create") {
            Ok(input) => crate::resource_syncs::preview_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Save the exact ResourceSync creation preview once with configuration-edit; enabling webhooks also requires operate. Revalidate name absence and private linked Repo baseline before one typed CreateResourceSync, then verify full saved configuration independently. Core requires an administrator and Read/Attach; no credential fallback. Saving triggers Core pending-cache refresh, source reads/Git-cache fetch and pending alerts through its sync admin. Submit NO RunSync, CommitSync, WriteSyncFileContents or deployment; external source write-back remains deferred. Uncertain acknowledgements consume the preview and are never retried.", input_schema = schema_for_input::<crate::resource_syncs::CreateApplyInput>().expect("apply_resource_sync_create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_resource_sync_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_resource_sync_create") {
            Ok(input) => crate::resource_syncs::apply_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview a typed partial ResourceSync definition edit with zero writes. Omitted fields and secret references are preserved; source variants deliberately clear conflicting selectors according to Core precedence host > linked Repo > Git > inline. Bind the complete private definition and active linked Repo, including secret-only changes. Show effective deletion (managed OR delete), empty-tag unrestricted resource scope, separate resource/variable/group flags, pending alerts and webhooks. Inline TOML is complete desired state, not a partial config patch; omitted desired fields can reset defaults. Applying execution-bearing settings needs both local grants even with currently disabled webhooks.", input_schema = schema_for_input::<crate::resource_syncs::PreviewInput>().expect("preview_resource_sync_update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_resource_sync_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_resource_sync_update") {
            Ok(input) => crate::resource_syncs::preview_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Save the exact target/patch and single-use preview_id once. Require configuration-edit and, for execution-bearing fields or enabling webhooks, operate before any Core I/O. Empty, pending-alert-only and webhook-disable-only patches need edit alone. Revalidate private definition and active linked Repo baselines, reject stale previews, and skip unchanged edits with zero writes. Submit one typed partial UpdateResourceSync, preserve omitted settings/secrets, and independently verify full saved config. Core Write/Attach decisions remain authoritative; its save hook refreshes pending/cache/source/alerts but submits no RunSync. No external Git/host write-back or automatic retry; final-read/write races and uncertain outcomes remain.", input_schema = schema_for_input::<crate::resource_syncs::ApplyInput>().expect("apply_resource_sync_update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_resource_sync_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_resource_sync_update") {
            Ok(input) => crate::resource_syncs::apply_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }
    #[tool(description="Export an explicit selection of at most 16 resources and 16 visible user groups through Core's scoped read API. targets is required; [] requires explicit groups or include_variables=true, which selects ALL Core variables, never per-variable names. Return parseable inspection-only TOML with conservative structural omission of arbitrary text, inline documents/scripts, credentials, all variable values and unknown fields, even for administrators. Identify missing definitions (Core omits inline ResourceSyncs), redactions and references outside the selection. Never use this incomplete export as authoritative desired state: omitted fields can reset existing configuration to defaults. No file write, cache refresh, RunSync or deployment; no export-all fallback. Bound source/output and reject excess without returning a truncated document.",input_schema=schema_for_input::<crate::export::Input>().expect("export_resources_toml schema"),output_schema=inventory::output_schema::<crate::export::Output>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn export_resources_toml(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "ExportResourcesToToml") {
            Ok(input) => crate::export::export(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="List Core Action/Procedure schedule owners in bounded positive pages. Core includes nonempty expressions even when disabled, and includes templates. Return configured expression/format/timezone/enabled and Core-reported cached next-run/error plus latest completed run start time (success or failure, manual or scheduled). A disabled/empty expression is not parsed; missing or cached values provide no validity/execution/health guarantee. Core internally collects/sorts owners before pagination.",input_schema=schema_for_input::<ListInput>().expect("list_schedules schema"),output_schema=inventory::output_schema::<crate::schedules::SchedulePage>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn list_schedules(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_schedules") {
            Ok(input) => crate::schedules::list(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Inspect the schedule of one exact Action/Procedure owner, including empty schedules and templates. Return saved fields plus a separate bounded Core ListSchedules cache lookup (at most four 100-item pages) matched by exact owner kind/ID and expected fields. Empty timezone inherits Core configured timezone, otherwise Core host local; never the MCP host. Cache/error/last-run evidence can be missing, stale or mismatched; no MCP parsing or scheduler.",input_schema=schema_for_input::<crate::schedules::DetailRequest>().expect("get_schedule schema"),output_schema=inventory::output_schema::<crate::schedules::Detail>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn get_schedule(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_schedule") {
            Ok(input) => crate::schedules::get(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Preview typed schedule fields for one saved Action/Procedure with configuration-edit and zero writes. Allow expression, format cron/english, timezone, enabled and alert only; omission preserves everything else. Bind the complete private parent source/stages/default arguments/secrets/settings in the shared expiring single-use preview. Empty expression clears the schedule and empty timezone inherits Core defaults. Every actual update requires both local grants; Core executes future trusted automation under administrator authority. Parsing and effective next run are Core-owned evidence, separate from saving.",input_schema=schema_for_input::<crate::schedules::PreviewInput>().expect("preview_schedule_update schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn preview_schedule_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_schedule_update") {
            Ok(input) => crate::schedules::preview(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Apply one previewed Action/Procedure schedule patch with both grants for every nonempty input plus Core owner Write. Consume the shared preview, recheck full private baseline, preserve source/stages/defaults/secrets/settings, handle no-op, submit one typed UpdateAction/UpdateProcedure and independently verify full saved config. Report Core parser/cache results separately; an invalid schedule may save successfully with a cache error. No CAS/retry/MCP scheduler. Disabling or clearing does not cancel already dispatched/running work; Core can reschedule from an older in-flight definition.",input_schema=schema_for_input::<crate::schedules::ApplyInput>().expect("apply_schedule_update schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn apply_schedule_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_schedule_update") {
            Ok(input) => crate::schedules::apply(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="List visible Actions in bounded positive pages. Return stable IDs, cached run state and latest completed run start time (successful or failed); source, default/per-run arguments and secrets are omitted. Multiple Core Action runs can overlap.",input_schema=schema_for_input::<ListInput>().expect("list_actions schema"),output_schema=inventory::output_schema::<Page<crate::actions::ActionSummary>>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn list_actions(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_actions") {
            Ok(input) => crate::actions::list(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Inspect one exact saved Action and its trusted execution effects without source, argument values or webhook secrets. Core runs TypeScript on its host with Deno --allow-all and a temporary built-in administrator API key. First read effects and copy effects.acknowledgement before running. This is trusted host/admin automation. Schedule tools are separate.",input_schema=schema_for_input::<DetailInput>().expect("get_action schema"),output_schema=inventory::output_schema::<crate::actions::Detail>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn get_action(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_action") {
            Ok(input) => crate::actions::get(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Preview inert Action creation with explicit trusted source and optional bounded JSON default_arguments. Zero writes. Force startup, webhooks and schedule disabled with empty schedule. Core requires administrator creation permission. Keep source/defaults private in the shared expiring single-use preview; no generic SDK or schedule passthrough.",input_schema=schema_for_input::<crate::actions::CreateInput>().expect("preview_action_create schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn preview_action_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_action_create") {
            Ok(input) => crate::actions::preview_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Create one previewed inert Action with configuration-edit and Core administrator permission. Recheck name absence, consume the shared single-use preview, send one typed write and verify echo plus full authoritative read-back. Startup/webhooks/schedule are disabled. No execution, retry or credential fallback.",input_schema=schema_for_input::<crate::actions::CreateApplyInput>().expect("apply_action_create schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn apply_action_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_action_create") {
            Ok(input) => crate::actions::apply_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Preview trusted Action source/default argument/execution-setting edits with zero writes. Omission preserves source, original argument format/string, secrets and schedule. default_arguments {} deliberately clears saved defaults and sets JSON format. Bind the full private definition; any actual existing-definition update requires both grants because Core refreshes scheduling. No schedule fields accepted.",input_schema=schema_for_input::<crate::actions::PreviewInput>().expect("preview_action_update schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn preview_action_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_action_update") {
            Ok(input) => crate::actions::preview_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Apply one previewed Action patch with configuration-edit plus operate for every nonempty update before Core I/O. Revalidate the full private baseline, preserve omissions, handle no-op, send one partial write and independently verify full saved config. Startup true enables execution on future Core starts; webhook/schedule races remain. No CAS, retry or RunAction submission.",input_schema=schema_for_input::<crate::actions::ApplyInput>().expect("apply_action_update schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn apply_action_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_action_update") {
            Ok(input) => crate::actions::apply_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Run one exact saved Action with operate/Core Execute and explicit acknowledgement of Core administrator/host authority. Submit stable ID once and optional bounded JSON argument overrides; Core recursively merges objects, appends arrays and replaces scalars, rejecting incompatible object/array overrides. Never rewrite saved source/defaults. Return the shared receipt and safe structured command evidence; arbitrary stdout/stderr is omitted because it can contain secret arguments/admin credentials. Wait/recovery never replays or attributes concurrent candidates; successful scripts do not prove workload health.",input_schema=schema_for_input::<crate::action_operations::RunInput>().expect("run_action schema"),output_schema=inventory::output_schema::<crate::operations::SubmissionReceipt>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn run_action(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "run_action") {
            Ok(input) => crate::action_operations::run(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Explicitly cancel one Action RunAction using required expected_update_id and operate/Core Execute. Validate selected Action, operation and update association in the handler AND shared transport; Core does not do this check. Always send the exact ID, allowing overlapping runs; never fall back to Core first-running-resource selection. Finished runs send nothing; token completion can race. Own cancellation ACK and original terminal state remain separate. Core kills the Deno process group, without rollback or cancellation of child Core operations. Local wait cancellation sends no remote cancellation.",input_schema=schema_for_input::<crate::action_operations::CancelInput>().expect("cancel_action schema"),output_schema=inventory::output_schema::<crate::operations::CancellationResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn cancel_action(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "cancel_action") {
            Ok(input) => crate::action_operations::cancel(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="List visible Procedures in bounded positive pages. Return stable IDs, configured stage count and cached run state; no scripts, execution arguments or secrets. Core stages are sequential and each enabled stage runs enabled executions in parallel.",input_schema=schema_for_input::<ListInput>().expect("list_procedures schema"),output_schema=inventory::output_schema::<Page<crate::procedures::ProcedureSummary>>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn list_procedures(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_procedures") {
            Ok(input) => crate::procedures::list(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Inspect one exact saved Procedure: ordered stages, parallel grouping, disabled entries, safe execution kinds/reference IDs and explicit truncation. Arguments and webhook secrets are omitted. Read effects and copy effects.acknowledgement before run_procedure. Core executes saved children as its built-in administrator user, not a caller-permission or Server-only Build sandbox. Schedule tools are separate.",input_schema=schema_for_input::<DetailInput>().expect("get_procedure schema"),output_schema=inventory::output_schema::<crate::procedures::Detail>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn get_procedure(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_procedure") {
            Ok(input) => crate::procedures::get(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Preview inert Procedure creation with trusted typed stages and zero writes. Explicit enabled flags preserve stage order/parallel grouping. Create requires stages and forces empty disabled schedule and disabled webhooks. Bind visible static references including disabled steps; Core checks editor Execute/Write/admin and creation permissions. Arguments remain private; no generic SDK passthrough.",input_schema=schema_for_input::<crate::procedures::CreateInput>().expect("preview_procedure_create schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn preview_procedure_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_procedure_create") {
            Ok(input) => crate::procedures::preview_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Create one previewed inert Procedure with configuration-edit and Core create/reference permissions; admin-only step types remain admin-only. Consume the shared single-use preview, revalidate references/name absence, submit one typed write and verify echo plus full authoritative read-back. Explicitly disable schedule/webhooks. Core refreshes schedule/cache; no RunProcedure is submitted. No retry or credential fallback.",input_schema=schema_for_input::<crate::procedures::CreateApplyInput>().expect("apply_procedure_create schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn apply_procedure_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_procedure_create") {
            Ok(input) => crate::procedures::apply_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Preview typed trusted Procedure edits with zero writes. Omission preserves ordered stages, disabled steps, arguments, secrets and schedule; [] deliberately clears stages. Supplied stages are complete typed replacements, not flattened operations. Privately bind the full current definition and bounded visible references. All nonempty updates require both local grants on apply because Core refreshes scheduling and policy can race. Scheduling fields are not accepted here.",input_schema=schema_for_input::<crate::procedures::PreviewInput>().expect("preview_procedure_update schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn preview_procedure_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_procedure_update") {
            Ok(input) => crate::procedures::preview_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Apply one previewed Procedure patch. Configuration-edit plus operate are required for every nonempty update before Core I/O. Revalidate private definition/references, preserve omitted secrets and unsupported existing steps during unrelated edits, handle no-op, send one partial write and verify full saved config independently. Core authoritatively checks editor referenced execution/admin permissions. No CAS, automatic retry or RunProcedure submission.",input_schema=schema_for_input::<crate::procedures::ApplyInput>().expect("apply_procedure_update schema"),output_schema=inventory::output_schema::<crate::configuration::ConfigurationResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn apply_procedure_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_procedure_update") {
            Ok(input) => crate::procedures::apply_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Run one exact saved Procedure with operate and Core Execute; first inspect get_procedure and acknowledge trusted Core administrator execution authority and broader effects. Submit its stable ID once without rewriting steps. Core executes stages sequentially and enabled children within a stage in parallel, under its built-in admin user. Return shared receipt, bounded safe stage/step reports and available reported child IDs; Finished execution does not prove child success. Definitions can race. Read-only wait/recovery never replays or attributes concurrent history candidates.",input_schema=schema_for_input::<crate::procedure_operations::RunInput>().expect("run_procedure schema"),output_schema=inventory::output_schema::<crate::operations::SubmissionReceipt>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn run_procedure(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "run_procedure") {
            Ok(input) => crate::procedure_operations::run(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Explicitly request cancellation of one saved Procedure with operate/Core Execute and locally checked expected_update_id of its original RunProcedure. Recheck nonterminal history and running state; completed/replaced/ambiguous runs send nothing. Core cancels by Procedure resource and IGNORES its optional update_id, so no atomic historical-run guard exists and replacements can race. Cancellation takes effect after the current stage finishes; no child cancellation or rollback is promised. Cancellation ACK and original terminal outcome remain separate. Cancelling a local wait sends no remote cancellation.",input_schema=schema_for_input::<crate::procedure_operations::CancelInput>().expect("cancel_procedure schema"),output_schema=inventory::output_schema::<crate::operations::CancellationResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn cancel_procedure(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "cancel_procedure") {
            Ok(input) => crate::procedure_operations::cancel(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Run exactly one saved Build by exact ID/name with operate and Core Execute. FIRST inspect get_build.configured_effects and copy its acknowledgement flags: trusted pre-build/Dockerfile/arguments, configured pushes and possible hidden/concurrent dependent redeployments. Revalidate the actual Build-to-Builder association and reject AWS/URL, templates or an empty Server list before submission, including in shared transport. Core chooses a cached-healthy least-used Server, with configured order breaking ties; the update does not attest the actual runner. Submit the stable Build ID once without config edits. Core can advance/store its version on success. Return shared receipt, bounded stages/revision/version and image output reports when available. Definitions can race after reads. Resume/reconcile read-only; never replay timeout/lost ACK or infer deployment health.",input_schema=schema_for_input::<crate::build_operations::RunInput>().expect("run_build schema"),output_schema=inventory::output_schema::<crate::operations::SubmissionReceipt>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn run_build(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "run_build") {
            Ok(input) => crate::build_operations::run(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Explicitly request cancellation of one saved Server-backed Build. Requires operate, Core Execute and expected_update_id of the selected original RunBuild. Validate that ID's target/operation, inspect current Server Builder, bounded nonterminal history and Core action state, and recheck before at most one CancelBuild. Already-finished, different/ambiguous or incomplete active evidence submits nothing. Core accepts ONLY the resource ID: it cannot atomically bind cancellation to this historical update, and a replacement can race after reads. Return cancellation ACK/update separately from the original run's latest observed outcome; a complete cancellation update only acknowledges queuing or cleanup. No rollback, automatic retry or remote cancellation from a local wait. Lost replies retain uncertainty; use read-only wait/reconciliation.",input_schema=schema_for_input::<crate::build_operations::CancelInput>().expect("cancel_build schema"),output_schema=inventory::output_schema::<crate::operations::CancellationResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn cancel_build(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "cancel_build") {
            Ok(input) => crate::build_operations::cancel(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "List visible Builders using bounded positive pages and name order. Identify Server/AWS/URL support without credentials. Direct writes and Build execution support explicit existing-Server builders only; AWS provisioning/configuration is deferred and URL writes are unsupported.", input_schema = schema_for_input::<ListInput>().expect("list_builders schema"), output_schema = inventory::output_schema::<Page<crate::builders::BuilderSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_builders(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_builders") {
            Ok(input) => crate::builders::list(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect one exact Builder by stable ID or case-sensitive name. Show ordered Server fallback IDs and explicit support limits. Unsupported AWS/URL definitions remain inspectable without addresses, passkeys, cloud credentials or secrets. Server order is meaningful; it is never sorted.", input_schema = schema_for_input::<DetailInput>().expect("get_builder schema"), output_schema = inventory::output_schema::<crate::builders::BuilderDetail>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_builder(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_builder") {
            Ok(input) => crate::builders::get(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview a Builder with config.kind=server and 1–16 ordered existing Server selectors. Reuse the shared ten-minute single-use preview store. Zero writes, builds or provisioning. Core requires an admin identity for creation and Read/Attach on the Servers; local edit access does not override that. Never inherit SDK AWS/URL defaults.", input_schema = schema_for_input::<crate::builders::CreateInput>().expect("preview_builder_create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_builder_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_builder_create") {
            Ok(input) => crate::builders::preview_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply exactly one previewed Server Builder creation with its preview_id. Revalidate name absence and ordered Server/trust references, submit one explicit Server variant, verify echo and independent entire configuration read-back. Core admin-only creation and Read/Attach remain authoritative. No cloud provisioning, Build execution or credential fallback; no automatic retry.", input_schema = schema_for_input::<crate::builders::CreateApplyInput>().expect("apply_builder_create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_builder_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_builder_create") {
            Ok(input) => crate::builders::apply_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview an explicit kind=server Builder patch with zero writes. Omit servers to preserve order or use [] to deliberately clear all Servers. Reject existing AWS/URL Builders without conversion. Bind private baseline and supplied Server/trust. Applying any supplied Server list needs both local grants because it can redirect Builds used by automatic execution; Core write hooks themselves perform no execution.", input_schema = schema_for_input::<crate::builders::PreviewInput>().expect("preview_builder_update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_builder_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_builder_update") {
            Ok(input) => crate::builders::preview_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact Server Builder target/patch/preview_id once. Applying servers requires both grants before I/O regardless of current automation policy. Revalidate authoritative Server variant, private baseline and supplied Server/trust, preserve ordering, and verify echo plus complete read-back after one write. Empty/no-op and stale proposals write zero times. Core Write/Attach and final-read/write races remain; no build, provision or retry.", input_schema = schema_for_input::<crate::builders::ApplyInput>().expect("apply_builder_update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_builder_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_builder_update") {
            Ok(input) => crate::builders::apply_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "List visible Build definitions with bounded positive pages and exact name order. Keep configured version/Builder association separate from current cached built/latest revisions. Return no scripts, Dockerfile/cache contents, arguments, tokens or credential names. This does not start builds.", input_schema = schema_for_input::<ListInput>().expect("list_builds schema"), output_schema = inventory::output_schema::<Page<crate::build_configuration::BuildSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_builds(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_builds") {
            Ok(input) => crate::build_configuration::list(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect one saved Build by exact ID/name with safe source/image/registry and trusted-hook presence. Inspect configured_effects and copy its acknowledgement flags before run_build; it discloses possible hidden/concurrent dependent redeployments. Identify its actual Builder support or an explicit observation gap. Configured source/version/image and cached observed revision are separate. AWS/URL Builders are inspectable but unsupported for direct definition writes and Build execution. Saving/inspection does not prove a build or healthy deployment.", input_schema = schema_for_input::<DetailInput>().expect("get_build schema"), output_schema = inventory::output_schema::<crate::build_configuration::BuildDetail>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_build(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_build") {
            Ok(input) => crate::build_configuration::get(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview a Build with explicit builder, source, image_name and image_registry ([] means local-only). Support typed Git, linked_repo, inline Dockerfile or host sources and existing-Server Builders only. Zero writes/executions. Bind private effective Builder/ordered Server/trust, linked Repo and visible credential evidence. Disclose pushes, visible dependent redeployment policy and Core's Git/Periphery cache refresh. Scripts/paths are trusted configuration; missing/hidden credential evidence is explicit.", input_schema = schema_for_input::<crate::build_configuration::CreateInput>().expect("preview_build_create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_build_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_build_create") {
            Ok(input) => crate::build_configuration::preview_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply exactly one previewed Build creation with shared preview_id. Git/linked-Repo/host source or enabled webhooks requires both grants before Core I/O; inline-only creation with webhooks explicitly off permits edit alone. Core creation and Builder/Repo Read-Attach remain authoritative. Revalidate references/name absence, save one typed partial config and verify echo plus complete read-back. Core can clone/pull source or read host Dockerfiles while refreshing caches, but MCP submits no RunBuild. No cloud provision, privileged fallback or retry.", input_schema = schema_for_input::<crate::build_configuration::CreateApplyInput>().expect("apply_build_create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_build_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_build_create") {
            Ok(input) => crate::build_configuration::apply_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview typed partial Build edits with zero writes. Omission preserves config/secrets; documented empty strings/lists clear. Sources select Git, linked_repo, inline Dockerfile or host and explicitly clear competing selectors. Inspect/revalidate the effective Server Builder and ordered Server/trust, linked Repo, visible credentials and dependent redeployment policy. All nonempty updates need both grants on apply: Core cache refresh can contact Git/Periphery and automation policy can race. No RunBuild is submitted.", input_schema = schema_for_input::<crate::build_configuration::PreviewInput>().expect("preview_build_update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_build_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_build_update") {
            Ok(input) => crate::build_configuration::preview_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact Build target/patch/shared preview_id once. Require both grants for every nonempty update before I/O; empty patches remain zero-write no-ops. Revalidate raw baseline, actual effective Server Builder, ordered Server/trust, linked Repo and visible credential evidence. Reject AWS/URL before mutation. Preserve omitted secrets and unrelated settings, verify echo and independent complete read-back. Core can refresh Git/host Dockerfile caches as its build identity. Core Write/Attach, no-CAS races and unknown outcomes remain; never retry or execute a Build.", input_schema = schema_for_input::<crate::build_configuration::ApplyInput>().expect("apply_build_update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_build_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_build_update") {
            Ok(input) => crate::build_configuration::apply_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "List visible Repos with bounded positive pages and exact-name ascending order. Return safe configured source/branch and current cached cloned revision separately; credential references, hooks and private URLs are omitted. Continue next_page with the same page_size.", input_schema = schema_for_input::<ListInput>().expect("list_repos schema"), output_schema = inventory::output_schema::<Page<crate::repositories::RepoSummary>>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn list_repos(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "list_repos") {
            Ok(input) => crate::repositories::list(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Inspect exactly one Repo by stable ID or exact case-sensitive name. Disclose saved on-clone/on-pull hook presence, environment, webhook policy and Server target without exposing commands or credentials. Inspect these trusted effects BEFORE clone/pull and copy the two hook flags into expected_hooks. Configured branch/pinned commit and cached observed cloned revision are separate; cached revision is not proof of a particular run.", input_schema = schema_for_input::<DetailInput>().expect("get_repo schema"), output_schema = inventory::output_schema::<crate::repositories::RepoDetail>(), annotations(read_only_hint = true, idempotent_hint = true, destructive_hint = false))]
    async fn get_repo(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "get_repo") {
            Ok(input) => crate::repositories::get(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview one Repo definition using explicit repo, branch and target plus allowlisted settings and credential references. Trusted configuration access is required. No writes or executions. Creation explicitly defaults webhooks off; enabling them on apply requires both local grants. Hooks and host destination paths are trusted executable configuration. The shared preview binds input, visible Server/trust and name absence; Core retains creation and Attach permission checks.", input_schema = schema_for_input::<crate::repositories::CreateInput>().expect("preview_repo_create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_repo_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_repo_create") {
            Ok(input) => crate::repositories::preview_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact previewed Repo creation and single-use preview_id once. Enabling webhooks requires both local grants. Revalidate visible name absence and Server/trust, submit one typed CreateRepo, verify the echo and independent authoritative complete configuration read-back. Core creation policy and Read/Attach still apply. No execution, credential fallback or retry; uncertainty consumes the preview.", input_schema = schema_for_input::<crate::repositories::CreateApplyInput>().expect("apply_repo_create schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_repo_create(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_repo_create") {
            Ok(input) => crate::repositories::apply_create(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Preview an allowlisted partial Repo edit with trusted configuration access and zero writes. Omission preserves fields and secrets; documented empty strings, empty hook commands and an unassigned target explicitly clear. Bind the private authoritative baseline and supplied Server reference/trust. Hook/environment edits are trusted code. Every execution-bearing update requires BOTH grants on apply, including unchanged supplied fields and currently disabled webhooks, because automatic execution policy can race. Edit-only access still permits previews, empty no-ops and webhook disabling. Core 2.3.2 definition writes refresh caches without clone/pull or old-target teardown.", input_schema = schema_for_input::<crate::repositories::PreviewInput>().expect("preview_repo_update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = true, idempotent_hint = false, destructive_hint = false))]
    async fn preview_repo_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "preview_repo_update") {
            Ok(input) => crate::repositories::preview_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Apply the exact Repo target/patch and shared preview_id once. Check grants before I/O, consume the preview, revalidate raw baseline and supplied references, then submit only supplied typed fields. No-op and stale previews write zero times. Applying source, target, credential, path, hook or environment fields requires operate plus configuration-edit independently of current webhook state; active or concurrently enabled webhooks can execute that configuration. Empty patches and webhook-disable-only patches need edit alone. Verify returned and independently read saved configuration while preserving omitted secrets. Core Write/Attach remains authoritative. A final-read/write race remains; never retry an ambiguous write or submit execution.", input_schema = schema_for_input::<crate::repositories::ApplyInput>().expect("apply_repo_update schema"), output_schema = inventory::output_schema::<crate::configuration::ConfigurationResult>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn apply_repo_update(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "apply_repo_update") {
            Ok(input) => crate::repositories::apply_update(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Clone exactly one saved Repo on its attached Server. Core first deletes its existing configured directory; uncommitted files can be lost. Periphery may write the configured environment file before hooks. Requires operate and Core Execute. FIRST inspect get_repo and acknowledge both saved hook-presence flags with expected_hooks: clone can run on-clone THEN on-pull and trusted environment/scripts can have broader effects. Reject changed hook presence before submission; concurrent Core definition changes still race. Execute by resolved stable ID at most once without rewriting definitions. Return shared update receipt and observed revision when available. Resume/reconcile read-only; never replay an ambiguous or timed-out call.", input_schema = schema_for_input::<crate::repositories::OperationInput>().expect("clone_repo schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn clone_repo(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "clone_repo") {
            Ok(input) => {
                crate::repositories::execute(
                    &self.client,
                    input,
                    crate::operations::WorkloadOperation::Clone,
                )
                .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Pull exactly one saved Repo on its attached Server, modifying its working tree and potentially overwriting the configured environment file before hooks. Requires operate and Core Execute. FIRST inspect get_repo and acknowledge both saved hook-presence flags with expected_hooks. Pull runs the configured on-pull hook; it does not run on-clone. Hooks/environment are trusted code. Reject changed hook presence before the one stable-ID submission; Core configuration can still race. Return shared receipt, bounded failed-stage evidence and observed revision when available. Timeout/lost acknowledgement never replays; get_update, wait_for_update and reconcile_operation remain read-only.", input_schema = schema_for_input::<crate::repositories::OperationInput>().expect("pull_repo schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn pull_repo(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "pull_repo") {
            Ok(input) => {
                crate::repositories::execute(
                    &self.client,
                    input,
                    crate::operations::WorkloadOperation::Pull,
                )
                .await
            }
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description="Find visible Server, Stack or Deployment records by one exact tag id or case-sensitive name. Resolve the tag to its stable ID, then use bounded positive pages and verify returned tag assignments. This remains read-only in every local profile. Hidden records and concurrent changes are not an exhaustive snapshot; an empty visible page does not prove no assignments.",input_schema=schema_for_input::<crate::tags::FindInput>().expect("tag discovery schema"),output_schema=inventory::output_schema::<crate::tags::FindResult>(),annotations(read_only_hint=true,idempotent_hint=true,destructive_hint=false))]
    async fn find_resources_by_tag(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "find_resources_by_tag") {
            Ok(input) => crate::tags::find(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }

    #[tool(description="Preview creating one exact case-sensitive tag with Core's default Slate color and zero writes. Trusted configuration access is required. Check exact name absence, retain a shared bounded ten-minute single-use preview, and disclose Core's caller ownership/non-admin creation policy. This does not assign a resource or execute a workload.",input_schema=schema_for_input::<crate::tags::CreateInput>().expect("tag create schema"),output_schema=inventory::output_schema::<crate::tags::TagResult>(),annotations(read_only_hint=true,idempotent_hint=false,destructive_hint=false))]
    async fn preview_tag_create(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "preview_tag_create") {
            Ok(input) => crate::tags::preview_create(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }
    #[tool(description="Apply the exact previewed tag name and preview_id once with trusted configuration access. Revalidate exact name absence before one typed CreateTag; verify the raw returned stable identity/name/color and authoritative read-back separately. Core assigns ownership and enforces creation policy. Lost/invalid/denied replies remain submission_unknown; no retry, resource assignment, execution or credential fallback.",input_schema=schema_for_input::<crate::tags::ApplyCreate>().expect("tag create apply schema"),output_schema=inventory::output_schema::<crate::tags::TagResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=false))]
    async fn apply_tag_create(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "apply_tag_create") {
            Ok(input) => crate::tags::apply_create(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }
    #[tool(description="Preview renaming one exact stable tag identity. Require trusted configuration access, preserve owner/color and assignment IDs, check exact name uniqueness and display the before/after name. An unchanged name is an explicit zero-write no-op. Raw identity/owner/color and intended name stay privately bound to the shared preview; Core requires tag owner or admin.",input_schema=schema_for_input::<crate::tags::RenameInput>().expect("tag rename schema"),output_schema=inventory::output_schema::<crate::tags::TagResult>(),annotations(read_only_hint=true,idempotent_hint=false,destructive_hint=false))]
    async fn preview_tag_rename(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "preview_tag_rename") {
            Ok(input) => crate::tags::preview_rename(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }
    #[tool(description="Apply the exact previewed tag rename once. Revalidate stable ID, private owner/color/name and destination-name absence; preserve assignment IDs. No-op submits zero writes. Separate typed RenameTag acknowledgement from exact read-back, keep transport uncertainty without retries, and retain Core tag-owner/admin authority. Discovery by the new name resolves the same stable tag ID.",input_schema=schema_for_input::<crate::tags::ApplyRename>().expect("tag rename apply schema"),output_schema=inventory::output_schema::<crate::tags::TagResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=false))]
    async fn apply_tag_rename(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "apply_tag_rename") {
            Ok(input) => crate::tags::apply_rename(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }
    #[tool(description="Preview the before/after exact tag sets for one Server, Stack or Deployment by kind/stable ID. Trusted configuration access is required. Omitted tags preserve assignments; replace with [] explicitly clears; mode=add unions requested resolved tags with existing assignments. Resolve exact IDs/names and bind private full resource metadata/configuration and tag definitions. Zero writes; at most32 requested/64 total tags. Core's metadata endpoint can create names or drop failed resolutions, so MCP sends only resolved IDs and discloses the final read-modify-write race.",input_schema=schema_for_input::<crate::tags::AssignInput>().expect("tag assignment schema"),output_schema=inventory::output_schema::<crate::tags::TagResult>(),annotations(read_only_hint=true,idempotent_hint=false,destructive_hint=false))]
    async fn preview_resource_tags(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "preview_resource_tags") {
            Ok(input) => crate::tags::preview_assign(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }
    #[tool(description="Apply the exact previewed resource kind/ID, mode, tag selectors and preview_id once. Revalidate raw resource metadata/configuration and resolved tag definitions. Omission/unchanged/add-existing are zero-write no-ops; replace[] clears deliberately. Submit only target and exact stable tag IDs through typed UpdateResourceMeta, preserving description/template/configuration. Core requires Write. Report acknowledgement separately from exact tags/unrelated-metadata read-back; concurrent deletion can still make Core drop an ID. No execute, automatic retry or credential fallback.",input_schema=schema_for_input::<crate::tags::ApplyAssign>().expect("tag assignment apply schema"),output_schema=inventory::output_schema::<crate::tags::TagResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=false))]
    async fn apply_resource_tags(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "apply_resource_tags") {
            Ok(input) => crate::tags::apply_assign(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }
    #[tool(description="Preview deliberate deletion of one exact tag with action=delete, using the shared destructive preview policy. Trusted configuration access is required; no writes. Identify its stable ID/name and bounded visible assignments across eleven resource families, disclose hidden/truncated/error gaps and Core's cross-resource assignment removal. Core 2.3.2 omits Swarm cleanup, so those assignments may remain dangling. Applying requires BOTH local grants and Core tag owner/admin, even if no visible assignment exists.",input_schema=schema_for_input::<crate::tags::DeleteInput>().expect("tag deletion schema"),output_schema=inventory::output_schema::<crate::tags::TagResult>(),annotations(read_only_hint=true,idempotent_hint=false,destructive_hint=true))]
    async fn preview_tag_delete(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "preview_tag_delete") {
            Ok(input) => crate::tags::preview_delete(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }
    #[tool(description="Deliberately delete the exact previewed tag identity with action=delete and preview_id once. BOTH startup grants are enforced by discovery/direct dispatch/shared transport; Core requires tag owner/admin and can remove assignments on hidden resources. Revalidate tag and visible assignment effects before one typed DeleteTag. Cross-collection cleanup can partially occur before failure; Core omits Swarm records. Report raw acknowledgement, exact tag-record read and visible assignment after-states separately. Never infer global cleanup from tag absence, retry uncertainty, execute workloads or repair unrelated records.",input_schema=schema_for_input::<crate::tags::ApplyDelete>().expect("tag deletion apply schema"),output_schema=inventory::output_schema::<crate::tags::TagResult>(),annotations(read_only_hint=false,idempotent_hint=false,destructive_hint=true))]
    async fn apply_tag_delete(&self, arguments: JsonObject) -> CallToolResult {
        let r = match inventory::parse(arguments, "apply_tag_delete") {
            Ok(input) => crate::tags::apply_delete(&self.client, input).await,
            Err(e) => Err(e),
        };
        inventory::tool_result(&self.client, r)
    }

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
