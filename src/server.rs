use rmcp::{
    ServerHandler,
    handler::server::tool::schema_for_input,
    model::{CallToolResult, Implementation, JsonObject, ServerCapabilities, ServerConfig},
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
                | "get_container_logs"
                | "get_deployment_logs"
                | "get_stack_logs"
                | "stack_diagnose" => Some(Capability::Read),
                "deploy_stack" | "deploy_deployment" => Some(Capability::Operate),
                _ => None,
            },
        )
    }
}

#[tool_router]
impl KomodoServer {
    #[tool(description = "Deploy one existing Stack from its saved definition and image-pull policy. Explicitly choose whole_stack or one exact Compose service; dependencies, saved hooks, flags and wrappers can have broader effects. Swarm supports whole-stack deploy only. Unlike restart, deploy can recreate containers with saved changes. Submit once, return an update receipt and one immediate bounded observation; acknowledgement is not rollout success. Never automatically retry execution after a timeout/lost reply: inspect the saved update ID/history. A new explicit call is a new non-idempotent operation.", input_schema = schema_for_input::<crate::operations::StackOperationInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn deploy_stack(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "deploy_stack") {
            Ok(input) => crate::operations::deploy_stack(&self.client, input).await,
            Err(error) => Err(error),
        };
        inventory::tool_result(&self.client, result)
    }

    #[tool(description = "Deploy one existing Deployment by stable ID or exact name, using saved configuration/image selection and Core's deploy image-pull behavior. Supports Server containers and Swarm services; no service filter or overrides. Deploy can recreate a container; restart uses existing containers. Submit at most once and return an update receipt with one immediate bounded observation. Acceptance does not establish execution/rollout success or workload health. Never automatically retry a lost/uncertain submission; inspect its update ID/history. A new explicit call can repeat effects.", input_schema = schema_for_input::<DetailInput>().expect("valid input schema"), output_schema = inventory::output_schema::<crate::operations::SubmissionReceipt>(), annotations(read_only_hint = false, idempotent_hint = false, destructive_hint = true))]
    async fn deploy_deployment(&self, arguments: JsonObject) -> CallToolResult {
        let result = match inventory::parse(arguments, "deploy_deployment") {
            Ok(input) => crate::operations::deploy_deployment(&self.client, input).await,
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
