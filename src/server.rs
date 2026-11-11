use rmcp::{
    ServerHandler,
    model::{Implementation, ServerCapabilities, ServerConfig},
    tool_handler, tool_router,
};

use crate::client::{CoreClient, CoreVersion};

pub struct KomodoServer {
    pub client: CoreClient,
    pub core_version: CoreVersion,
}

// Register future tools in this impl; the SDK routes both listing and calls.
#[tool_router(allow_empty)]
impl KomodoServer {}

#[tool_handler]
impl ServerHandler for KomodoServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build()).with_server_info(
            Implementation::new(env!("CARGO_PKG_NAME"), env!("CARGO_PKG_VERSION")),
        )
    }
}
