//! Private bounded snapshots of immediate typed dependencies and their graph.
use super::*;
use std::collections::VecDeque;

const MAX_REFERENCES: usize = 32;
fn dependencies(kind: Kind, config: &Value) -> Result<Vec<(Kind, String)>, AppError> {
    let mut out = Vec::new();
    let mut one = |kind: Kind, field: &str| -> Result<(), AppError> {
        if let Some(v) = config.get(field)
            && !v.is_null()
        {
            let s = v.as_str().ok_or_else(invalid)?;
            if !s.is_empty() {
                if !name_ok(s) {
                    return Err(invalid());
                }
                out.push((kind, s.into()));
            }
        }
        Ok(())
    };
    match kind {
        Kind::Stack | Kind::Deployment => {
            one(Kind::Server, "server_id")?;
            one(Kind::Swarm, "swarm_id")?;
            if kind == Kind::Stack {
                one(Kind::Repo, "linked_repo")?;
            }
        }
        Kind::Build => {
            one(Kind::Builder, "builder_id")?;
            one(Kind::Repo, "linked_repo")?;
        }
        Kind::Repo => {
            one(Kind::Server, "server_id")?;
            one(Kind::Builder, "builder_id")?;
        }
        Kind::ResourceSync => {
            one(Kind::Repo, "linked_repo")?;
        }
        _ => (),
    }
    if kind == Kind::Deployment && config["image"]["type"] == "Build" {
        let id = config["image"]["params"]["build_id"]
            .as_str()
            .ok_or_else(invalid)?;
        if !name_ok(id) {
            return Err(invalid());
        }
        out.push((Kind::Build, id.into()));
    }
    let servers = if kind == Kind::Swarm {
        config.get("server_ids")
    } else if kind == Kind::Builder && config["type"] == "Server" {
        config["params"].get("server_ids")
    } else {
        None
    };
    if let Some(ids) = servers.filter(|v| !v.is_null()) {
        let ids = ids.as_array().ok_or_else(invalid)?;
        if ids.len() > 16 {
            return Err(invalid());
        }
        for id in ids {
            let id = id.as_str().ok_or_else(invalid)?;
            if !name_ok(id) {
                return Err(invalid());
            }
            out.push((Kind::Server, id.into()));
        }
    }
    // Audit explicit Alerter targets too. Never infer references from arbitrary
    // strings, Action code, hooks or Procedure log output.
    if kind == Kind::Alerter
        && let Some(items) = config.get("resources").filter(|v| !v.is_null())
    {
        let items = items.as_array().ok_or_else(invalid)?;
        if items.len() > 32 {
            return Err(invalid());
        }
        for item in items {
            let kind = match item["type"].as_str() {
                Some("Server") => Kind::Server,
                Some("Swarm") => Kind::Swarm,
                Some("Stack") => Kind::Stack,
                Some("Deployment") => Kind::Deployment,
                Some("Build") => Kind::Build,
                Some("Repo") => Kind::Repo,
                Some("Builder") => Kind::Builder,
                Some("Alerter") => Kind::Alerter,
                Some("Procedure") => Kind::Procedure,
                Some("Action") => Kind::Action,
                Some("ResourceSync") => Kind::ResourceSync,
                _ => return Err(invalid()),
            };
            let id = item["id"].as_str().ok_or_else(invalid)?;
            if !name_ok(id) {
                return Err(invalid());
            }
            out.push((kind, id.into()));
        }
    }
    if kind == Kind::Procedure
        && let Some(stages) = config.get("stages").filter(|v| !v.is_null())
    {
        let stages = stages.as_array().ok_or_else(invalid)?;
        if stages.len() > 16 {
            return Err(invalid());
        }
        let mut count = 0;
        for stage in stages {
            let executions = stage["executions"].as_array().ok_or_else(invalid)?;
            count += executions.len();
            if count > 64 {
                return Err(invalid());
            }
            for execution in executions {
                let execution = &execution["execution"];
                // Include disabled steps: saving can also enable automatic
                // execution, and source omission has desired-state defaults.
                if execution["type"] == "CommitSync" {
                    return Err(AppError {
                        correction: Some(
                            "Known CommitSync Procedure steps, including disabled steps, cannot authorize external Git/host write-back through reviewed ResourceSync application. External source write-back is deferred.",
                        ),
                        ..AppError::new(ErrorKind::UnsupportedTarget, OP, None)
                    });
                }
                let params = &execution["params"];
                for (field, kind) in [
                    ("build", Kind::Build),
                    ("repo", Kind::Repo),
                    ("action", Kind::Action),
                    ("procedure", Kind::Procedure),
                    ("deployment", Kind::Deployment),
                    ("stack", Kind::Stack),
                    ("server", Kind::Server),
                    ("swarm", Kind::Swarm),
                    ("sync", Kind::ResourceSync),
                    ("alerter", Kind::Alerter),
                ] {
                    if let Some(id) = params.get(field) {
                        let id = id.as_str().ok_or_else(invalid)?;
                        if !name_ok(id) {
                            return Err(invalid());
                        }
                        out.push((kind, id.into()));
                    }
                }
                if execution["type"] == "SendAlert"
                    && let Some(items) = params.get("alerters")
                {
                    let items = items.as_array().ok_or_else(invalid)?;
                    if items.len() > 32 {
                        return Err(invalid());
                    }
                    for id in items {
                        let id = id.as_str().ok_or_else(invalid)?;
                        if !name_ok(id) {
                            return Err(invalid());
                        }
                        out.push((Kind::Alerter, id.into()));
                    }
                }
            }
        }
    }
    Ok(out)
}
fn order(kind: Kind) -> usize {
    match kind {
        Kind::Server => 0,
        Kind::Alerter => 1,
        Kind::Action => 2,
        Kind::Swarm => 3,
        Kind::Builder => 4,
        Kind::Repo => 5,
        Kind::Build => 6,
        Kind::Stack => 7,
        Kind::ResourceSync => 8,
        Kind::Deployment => 9,
        Kind::Procedure => 10,
    }
}
pub(super) async fn collect(
    client: &CoreClient,
    scope: &ExecutionScope,
    rows: &[Change],
    configs: Vec<(Kind, String, Value)>,
    source: Value,
) -> Result<Value, AppError> {
    let selected = |kind: Kind, name: &str| match scope {
        ExecutionScope::WholeSync {} => true,
        ExecutionScope::ResourceType { resource_type } => *resource_type == kind,
        ExecutionScope::Resources {
            resource_type,
            targets,
        } => {
            *resource_type == kind
                && targets.iter().any(|t| match t {
                    ResolvedTarget::Create { name: n }
                    | ResolvedTarget::Existing { name: n, .. } => n == name,
                })
        }
    };
    let mut queue = VecDeque::new();
    for (kind, _, config) in &configs {
        for (dependency, id) in dependencies(*kind, config)? {
            queue.push_back((*kind, dependency, id));
        }
    }
    let mut seen = BTreeSet::new();
    let mut resources = Vec::new();
    while let Some((parent, kind, id)) = queue.pop_front() {
        let creating = !valid_id(&id)
            && rows.iter().any(|r| {
                r.resource_type == Some(kind)
                    && r.action == ChangeKind::Create
                    && r.name.as_deref() == Some(&id)
            });
        // Validate each edge before deduplicating the resource snapshot. A
        // dependency first encountered under a later-executed parent must not
        // authorize the same dependency for an earlier-executed parent.
        if creating && (!selected(kind, &id) || order(kind) >= order(parent)) {
            return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
        }
        if !seen.insert((kind, id.clone())) {
            continue;
        }
        if seen.len() > MAX_REFERENCES {
            return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
        }
        if creating {
            resources
                .push(json!({"kind":kind,"source_create":id,"identity_not_yet_assigned":true}));
            continue;
        }
        let target = if valid_id(&id) {
            DetailInput {
                id: Some(id),
                name: None,
            }
        } else {
            DetailInput {
                id: None,
                name: Some(id),
            }
        };
        let baseline = super::audit::read_resource(client, kind, target).await?;
        if baseline["template"] == true {
            return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
        }
        for (dependency, id) in dependencies(kind, &baseline["config"])? {
            queue.push_back((kind, dependency, id));
        }
        resources.push(baseline);
    }
    resources.sort_by_cached_key(Value::to_string);
    Ok(json!({"source":source,"desired_dependencies":resources}))
}
