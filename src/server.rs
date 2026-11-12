use rmcp::{
    ServerHandler,
    handler::server::tool::schema_for_input,
    model::{CallToolResult, Implementation, JsonObject, ServerCapabilities, ServerConfig},
    tool, tool_handler, tool_router,
};

use crate::{
    client::{CoreClient, CoreVersion},
    inventory::{self, *},
};

pub struct KomodoServer {
    pub client: CoreClient,
    pub core_version: CoreVersion,
}

#[tool_router]
impl KomodoServer {
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
}

#[tool_handler]
impl ServerHandler for KomodoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        )
    }
}
