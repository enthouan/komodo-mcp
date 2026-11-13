//! Bounded visible structured references, never a claim of complete visibility.
use std::collections::BTreeSet;

use komodo_client::api::read::{
    GetAlerter, GetBuilder, GetDeployment, GetRepo, GetStack, ListFullAlerters, ListFullBuilders,
    ListFullDeployments, ListFullRepos, ListFullStacks,
};
use serde_json::{Value, json};

use super::{Coverage, Reference, ResourceKind, Target, invalid, object_id, raw_id};
use crate::{
    client::CoreClient,
    configuration::bounded_text,
    error::{AppError, ErrorKind},
};

const LIMIT: u64 = 20;
const PAGES: u64 = 2;
pub(super) const DISPLAY_LIMIT: usize = 32;

pub(super) struct Scan {
    pub references: Vec<Reference>,
    pub coverage: Vec<Coverage>,
    pub snapshot: Value,
}
struct Family {
    references: Vec<Reference>,
    coverage: Coverage,
    raw: Vec<Value>,
}

fn fields<'a>(config: &'a Value, family: &str) -> Result<Vec<(&'static str, &'a Value)>, AppError> {
    let field = |key: &str| config.get(key).ok_or_else(invalid);
    match family {
        "alerter" => {
            let a = field("resources")?;
            let b = field("except_resources")?;
            for v in [a, b] {
                serde_json::from_value::<Vec<komodo_client::entities::ResourceTarget>>(v.clone())
                    .map_err(|_| invalid())?;
            }
            Ok(vec![
                ("config.resources", a),
                ("config.except_resources", b),
            ])
        }
        "builder" => match config["type"].as_str() {
            Some("Server") => {
                let ids = config["params"]
                    .get("server_ids")
                    .filter(|v| {
                        v.as_array().is_some_and(|v| {
                            v.iter()
                                .all(|id| id.as_str().is_some_and(|s| s.is_empty() || object_id(s)))
                        })
                    })
                    .ok_or_else(invalid)?;
                Ok(vec![("config.params.server_ids", ids)])
            }
            Some("Url" | "Aws") => Ok(vec![]),
            _ => Err(invalid()),
        },
        "stack" | "deployment" | "repo" => {
            let v = field("server_id")?;
            if !v.as_str().is_some_and(|s| s.is_empty() || object_id(s)) {
                return Err(invalid());
            }
            Ok(vec![("config.server_id", v)])
        }
        _ => Err(invalid()),
    }
}
fn matches(value: &Value, family: &str, target: &Target) -> bool {
    if family == "alerter" {
        value.as_array().is_some_and(|items| {
            items.iter().any(|v| {
                v["type"] == target.kind.core_name()
                    && v["id"]
                        .as_str()
                        .is_some_and(|id| id.eq_ignore_ascii_case(&target.id))
            })
        })
    } else if family == "builder" {
        value.as_array().is_some_and(|items| {
            items.iter().any(|v| {
                v.as_str()
                    .is_some_and(|id| id.eq_ignore_ascii_case(&target.id))
            })
        })
    } else {
        value
            .as_str()
            .is_some_and(|id| id.eq_ignore_ascii_case(&target.id))
    }
}
async fn page(client: &CoreClient, family: &str, page: u64) -> Result<Value, AppError> {
    macro_rules! read {
        ($r:ident) => {
            client
                .read_references($r {
                    page,
                    limit: Some(LIMIT),
                    ..Default::default()
                })
                .await
        };
    }
    match family {
        "alerter" => read!(ListFullAlerters),
        "builder" => read!(ListFullBuilders),
        "repo" => read!(ListFullRepos),
        "stack" => read!(ListFullStacks),
        "deployment" => read!(ListFullDeployments),
        _ => Err(invalid()),
    }
}
async fn family(client: &CoreClient, target: &Target, name: &str) -> Result<Family, AppError> {
    let mut f = Family {
        references: vec![],
        raw: vec![],
        coverage: Coverage {
            family: name.into(),
            visible_records_examined: 0,
            visible_scan_complete: false,
            hidden_records_possible: true,
            error: None,
        },
    };
    let mut seen = BTreeSet::new();
    for n in 0..PAGES {
        let value = match page(client, name, n).await {
            Ok(v) => v,
            Err(e) if e.kind == ErrorKind::InvalidResponse => return Err(e),
            Err(e) => {
                f.coverage.error = Some(e);
                break;
            }
        };
        let items = value
            .as_array()
            .filter(|v| v.len() <= LIMIT as usize)
            .ok_or_else(invalid)?;
        for record in items {
            let id = raw_id(record)?;
            let resource_name = record["name"]
                .as_str()
                .filter(|s| !s.trim().is_empty())
                .ok_or_else(invalid)?;
            if !seen.insert(id.clone())
                || !record["template"].is_boolean()
                || !record["config"].is_object()
            {
                return Err(invalid());
            }
            f.coverage.visible_records_examined += 1;
            for (field, value) in fields(&record["config"], name)? {
                if !matches(value, name, target) {
                    continue;
                }
                f.raw.push(json!({"kind":name,"id":id,"name":resource_name,"template":record["template"],"field":field,"reference":value}));
                f.references.push(Reference {kind:name.into(),id:id.clone(),name:bounded_text(client,resource_name,256),field:field.into(),
                    expected_effect:if name=="builder" {"Core targets the legacy scalar server_id field; current server_ids references can remain dangling"}else{"Core removes this structured reference; unrelated values should remain"}.into(),after:None,error:None});
            }
        }
        if items.len() < LIMIT as usize {
            f.coverage.visible_scan_complete = true;
            break;
        }
    }
    Ok(f)
}

pub(super) async fn scan(client: &CoreClient, target: &Target) -> Result<Scan, AppError> {
    let mut families = vec![family(client, target, "alerter").await?];
    if target.kind == ResourceKind::Server {
        let (s, d, r, b) = tokio::join!(
            family(client, target, "stack"),
            family(client, target, "deployment"),
            family(client, target, "repo"),
            family(client, target, "builder")
        );
        families.extend([s?, d?, r?, b?]);
    }
    let mut refs = vec![];
    let mut coverage = vec![];
    let mut raw = vec![];
    for f in families {
        refs.extend(f.references);
        coverage.push(f.coverage);
        raw.extend(f.raw);
    }
    refs.sort_by(|a, b| (&a.kind, &a.id, &a.field).cmp(&(&b.kind, &b.id, &b.field)));
    raw.sort_by_key(Value::to_string);
    let snapshot = json!({"references":raw,"coverage":coverage});
    Ok(Scan {
        references: refs,
        coverage,
        snapshot,
    })
}

async fn get(client: &CoreClient, r: &Reference) -> Result<Value, AppError> {
    match r.kind.as_str() {
        "alerter" => {
            client
                .read_references(GetAlerter {
                    alerter: r.id.clone(),
                })
                .await
        }
        "builder" => {
            client
                .read_references(GetBuilder {
                    builder: r.id.clone(),
                })
                .await
        }
        "repo" => client.read_references(GetRepo { repo: r.id.clone() }).await,
        "stack" => {
            client
                .read_references(GetStack {
                    stack: r.id.clone(),
                })
                .await
        }
        "deployment" => {
            client
                .read_references(GetDeployment {
                    deployment: r.id.clone(),
                })
                .await
        }
        _ => Err(invalid()),
    }
}
pub(super) async fn observe(client: &CoreClient, target: &Target, references: &mut [Reference]) {
    for r in references {
        let observed = async {
            let raw = get(client, r).await?;
            if raw_id(&raw)? != r.id || !raw["config"].is_object() {
                return Err(invalid());
            }
            let still_present = fields(&raw["config"], &r.kind)?
                .iter()
                .any(|(field, v)| *field == r.field && matches(v, &r.kind, target));
            Ok::<_, AppError>(if still_present {
                "still_referencing_target"
            } else {
                "no_longer_referencing_target"
            })
        }
        .await;
        match observed {
            Ok(v) => r.after = Some(v.into()),
            Err(e) => {
                r.after = Some(
                    if e.kind == ErrorKind::NotFound {
                        "resource_not_found"
                    } else {
                        "observation_unavailable"
                    }
                    .into(),
                );
                r.error = Some(e);
            }
        }
    }
}
