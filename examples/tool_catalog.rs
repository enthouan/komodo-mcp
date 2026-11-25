//! Generate the website's complete built-in tool catalog offline from registered metadata.
#[path = "support/schema_pool.rs"]
mod schema_pool;
use schema_pool::SchemaPool;

use std::{collections::BTreeMap, env, error::Error, fs};

use komodo_mcp::{
    capabilities::{Capability, CapabilityPolicy},
    inventory::ToolOutput,
    server::KomodoServer,
    terminal,
};
use rmcp::{handler::server::tool::schema_for_output, model::ToolAnnotations};
use serde::Serialize;
use serde_json::Value;

const CATALOG: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/docs/tool-catalog.json");
const GRANTS: [&str; 4] = [
    "KOMODO_ALLOW_OPERATE",
    "KOMODO_ALLOW_CONFIGURATION_EDIT",
    "KOMODO_ALLOW_PRUNE",
    "KOMODO_ALLOW_TERMINAL",
];

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Entry {
    description: String,
    grant_sets: Vec<Vec<&'static str>>,
    annotations: ToolAnnotations,
    input_schema: String,
    output_schema: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_source: Option<&'static str>,
}

#[derive(Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct Catalog {
    tools: BTreeMap<String, Entry>,
    #[serde(flatten)]
    pool: SchemaPool,
}

// Derive minimum qualifying grant combinations from the actual runtime policy.
// Input-dependent checks and Core's decisions still apply after discovery.
fn grant_sets(capability: Capability) -> Vec<Vec<&'static str>> {
    let allowed: Vec<u8> = (0..16)
        .filter(|mask| {
            CapabilityPolicy::new(mask & 1 != 0, mask & 2 != 0)
                .with_pruning(mask & 4 != 0)
                .with_terminal(mask & 8 != 0)
                .allows(capability)
        })
        .collect();
    allowed
        .iter()
        .filter(|&&mask| {
            !allowed
                .iter()
                .any(|&other| other != mask && other & mask == other)
        })
        .map(|&mask| {
            GRANTS
                .iter()
                .enumerate()
                .filter_map(|(bit, &name)| (mask & (1 << bit) != 0).then_some(name))
                .collect()
        })
        .collect()
}

fn build() -> Result<Catalog, Box<dyn Error>> {
    let mut catalog = Catalog::default();
    let mut tools = KomodoServer::built_in_tools();
    tools.sort_by(|(a, _), (b, _)| a.name.cmp(&b.name));
    for (tool, capability) in tools {
        let name = tool.name.into_owned();
        let capability =
            capability.ok_or_else(|| format!("Unclassified registered tool {name}"))?;
        let description = tool
            .description
            .ok_or_else(|| format!("Missing description for {name}"))?
            .into_owned();
        let annotations = tool
            .annotations
            .ok_or_else(|| format!("Missing effect annotations for {name}"))?;
        let (output, output_source) = match tool.output_schema {
            Some(schema) => (Value::Object(schema.as_ref().clone()), None),
            None if name == "execute_terminal" => {
                let mut schema = schema_for_output::<ToolOutput<terminal::Output>>()
                    .as_ref()
                    .clone();
                schema.insert("type".into(), Value::String("object".into()));
                (Value::Object(schema), Some("resultType"))
            }
            None => {
                return Err(format!("No registered or documented output shape for {name}").into());
            }
        };
        let input_schema = catalog.pool.intern(
            format!("{name}.input"),
            Value::Object(tool.input_schema.as_ref().clone()),
        )?;
        let output_schema = catalog.pool.intern(format!("{name}.output"), output)?;
        let entry = Entry {
            description,
            grant_sets: grant_sets(capability),
            annotations,
            input_schema,
            output_schema,
            output_source,
        };
        if catalog.tools.insert(name.clone(), entry).is_some() {
            return Err(format!("Duplicate registered tool {name}").into());
        }
    }
    Ok(catalog)
}

fn generate() -> Result<String, Box<dyn Error>> {
    Ok(serde_json::to_string_pretty(&build()?)? + "\n")
}

fn check() -> Result<(), Box<dyn Error>> {
    if generate()? != fs::read_to_string(CATALOG)? {
        return Err(
            "Tool catalog drift. Run cargo run --locked --example tool_catalog -- --write".into(),
        );
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    match env::args().skip(1).collect::<Vec<_>>().as_slice() {
        [] => check(),
        [option] if option == "--write" => {
            fs::write(CATALOG, generate()?)?;
            Ok(())
        }
        _ => Err("Usage: cargo run --locked --example tool_catalog -- [--write]".into()),
    }
}

#[test]
fn committed_catalog_matches_the_full_runtime_registry() {
    check().unwrap();
}

#[test]
fn expanded_schemas_equal_every_document_advertised_by_the_router() {
    let catalog = build().unwrap();
    let expand = |id: &str| {
        let mut schema = catalog.pool.schemas[id].clone();
        if let Some(names) = catalog.pool.schema_definitions.get(id) {
            schema["$defs"] = Value::Object(
                names
                    .iter()
                    .map(|(local, shared)| {
                        (local.clone(), catalog.pool.definitions[shared].clone())
                    })
                    .collect(),
            );
        }
        schema
    };
    for (tool, _) in KomodoServer::built_in_tools() {
        let entry = &catalog.tools[tool.name.as_ref()];
        assert_eq!(
            expand(&entry.input_schema),
            Value::Object(tool.input_schema.as_ref().clone()),
            "{} input",
            tool.name
        );
        if let Some(output) = tool.output_schema {
            assert_eq!(
                expand(&entry.output_schema),
                Value::Object(output.as_ref().clone()),
                "{} output",
                tool.name
            );
        } else {
            assert_eq!(tool.name, "execute_terminal");
            assert_eq!(entry.output_source, Some("resultType"));
        }
    }
}

#[test]
fn catalog_grants_match_every_runtime_profile() {
    let catalog = build().unwrap();
    let tools = KomodoServer::built_in_tools();
    for mask in 0..16 {
        let enabled: Vec<_> = GRANTS
            .into_iter()
            .enumerate()
            .filter_map(|(bit, grant)| (mask & (1 << bit) != 0).then_some(grant))
            .collect();
        let policy = CapabilityPolicy::new(
            enabled.contains(&"KOMODO_ALLOW_OPERATE"),
            enabled.contains(&"KOMODO_ALLOW_CONFIGURATION_EDIT"),
        )
        .with_pruning(enabled.contains(&"KOMODO_ALLOW_PRUNE"))
        .with_terminal(enabled.contains(&"KOMODO_ALLOW_TERMINAL"));
        for (tool, capability) in &tools {
            let entry = &catalog.tools[tool.name.as_ref()];
            let visible = entry
                .grant_sets
                .iter()
                .any(|grants| grants.iter().all(|grant| enabled.contains(grant)));
            assert_eq!(
                visible,
                policy.allows(capability.unwrap()),
                "{} under {enabled:?}",
                tool.name
            );
        }
    }
}
