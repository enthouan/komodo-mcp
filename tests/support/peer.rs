//! Drive the production MCP executable through either supported transport.
#![allow(dead_code)]
use crate::{
    http::{Binary, DEADLINE, FIRST},
    support::operations::{Process, Session},
};
use rmcp::{
    ErrorData,
    model::{CallToolRequestParams, CallToolResult, ListToolsResult, Tool},
    service::ServiceError,
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet};
use tokio::time::timeout;

#[derive(Clone, Copy)]
pub enum Transport {
    Stdio,
    Http,
}
pub const TRANSPORTS: [Transport; 2] = [Transport::Stdio, Transport::Http];

pub enum Peer {
    Stdio(Process, Box<Session>),
    Http(Binary, String),
}

impl Transport {
    pub async fn connect(self, address: &str, settings: &[(&str, &str)]) -> Peer {
        match self {
            Self::Stdio => {
                let (process, session) = Process::start_with_settings(address, settings).await;
                Peer::Stdio(process, Box::new(session))
            }
            Self::Http => {
                let process = Binary::start(address, settings).await;
                let session = process.http.session(FIRST, None).await;
                Peer::Http(process, session)
            }
        }
    }
}

impl Peer {
    pub async fn catalog(&self) -> BTreeMap<String, Tool> {
        let tools = match self {
            Self::Stdio(_, s) => timeout(DEADLINE, s.list_all_tools())
                .await
                .unwrap()
                .unwrap(),
            Self::Http(p, s) => {
                let mut tools = Vec::new();
                let mut cursors = BTreeSet::new();
                let mut params = json!({});
                loop {
                    let response = p.http.rpc(FIRST, s, "tools/list", params).await;
                    let page: ListToolsResult =
                        serde_json::from_value(response["result"].clone()).unwrap();
                    tools.extend(page.tools);
                    let Some(cursor) = page.next_cursor else {
                        break;
                    };
                    assert!(
                        cursors.insert(cursor.clone()),
                        "repeated cursor in discovery"
                    );
                    params = json!({"cursor":cursor});
                }
                tools
            }
        };
        let mut result = BTreeMap::new();
        for tool in tools {
            assert!(
                result.insert(tool.name.to_string(), tool).is_none(),
                "duplicate advertised tool"
            );
        }
        result
    }

    pub async fn call(&self, name: &str, args: Value) -> Result<CallToolResult, ErrorData> {
        match self {
            Self::Stdio(_, s) => {
                let req = CallToolRequestParams::new(name.to_owned())
                    .with_arguments(args.as_object().unwrap().clone());
                match timeout(DEADLINE, s.call_tool(req)).await.unwrap() {
                    Ok(result) => Ok(result),
                    Err(ServiceError::McpError(error)) => Err(error),
                    Err(other) => panic!("unexpected stdio transport failure: {other}"),
                }
            }
            Self::Http(p, s) => {
                let response = p.http.call(FIRST, s, name, args).await;
                if let Some(error) = response.get("error") {
                    Err(serde_json::from_value(error.clone()).unwrap())
                } else {
                    Ok(serde_json::from_value(response["result"].clone()).unwrap())
                }
            }
        }
    }

    pub async fn data(&self, name: &str, args: Value) -> Value {
        let call = self.call(name, args).await.unwrap();
        assert_eq!(call.is_error, Some(false));
        call.structured_content.unwrap()
    }

    pub async fn finish(self) {
        match self {
            Self::Stdio(p, s) => p.finish(*s).await,
            Self::Http(p, _) => p.finish().await,
        }
    }
}
