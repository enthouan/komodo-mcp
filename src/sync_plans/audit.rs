//! Coverage checks for differences between pinned Core's view and execution.
use super::*;
use komodo_client::{
    api::read::{
        GetAction, GetAlerter, GetBuild, GetBuilder, GetDeployment, GetProcedure, GetRepo,
        GetServer, GetStack, GetSwarm, ListVariables,
    },
    entities::{
        sync::ResourceSyncConfig, to_docker_compatible_name, to_general_name,
        to_path_compatible_name, toml::ResourceToml,
    },
};

// No Debug/Serialize: source documents and reference baselines contain secrets.
struct DesiredResource {
    kind: Kind,
    name: String,
    template: bool,
    deploy: bool,
    after: Vec<String>,
}
struct Desired {
    resources: Vec<DesiredResource>,
    variables: Vec<(String, bool)>,
    groups: Vec<String>,
    deploy_requested: bool,
}
struct Metadata {
    id: String,
    name: String,
    template: bool,
}

pub(super) fn validate_selection(selection: &Selection) -> Result<(), AppError> {
    if let Selection::Resources { targets, .. } = selection {
        if targets.is_empty() || targets.len() > MAX_SELECTORS {
            return Err(AppError::arguments(
                OP,
                "Select 1–16 exact targets within one explicit resource type; an empty or invalid selection never becomes an unfiltered sync",
            ));
        }
        for t in targets {
            export::validate_selector(t)?;
        }
    }
    Ok(())
}
fn selected(scope: &ExecutionScope, kind: Kind, id: Option<&str>, name: &str) -> bool {
    match scope {
        ExecutionScope::WholeSync {} => true,
        ExecutionScope::ResourceType { resource_type } => *resource_type == kind,
        ExecutionScope::Resources {
            resource_type,
            targets,
        } => {
            *resource_type == kind
                && targets.iter().any(|t| match t {
                    ResolvedTarget::Existing { id: target_id, .. } => id == Some(target_id),
                    ResolvedTarget::Create { name: target_name } => {
                        id.is_none() && name == target_name
                    }
                })
        }
    }
}
fn source_selected(scope: &ExecutionScope, resource: &DesiredResource) -> bool {
    match scope {
        ExecutionScope::WholeSync {} => true,
        ExecutionScope::ResourceType { resource_type } => *resource_type == resource.kind,
        ExecutionScope::Resources {
            resource_type,
            targets,
        } => {
            *resource_type == resource.kind
                && targets.iter().any(|t| match t {
                    ResolvedTarget::Existing { name, .. } | ResolvedTarget::Create { name } => {
                        name == &resource.name
                    }
                })
        }
    }
}
fn has_whole(scope: &ExecutionScope) -> bool {
    matches!(scope, ExecutionScope::WholeSync {})
}
fn add_resources<C: Default>(
    out: &mut Vec<DesiredResource>,
    rows: &[ResourceToml<C>],
    kind: Kind,
    config: &ResourceSyncConfig,
) {
    for row in rows {
        if config.match_tags.iter().all(|tag| row.tags.contains(tag)) {
            out.push(DesiredResource {
                kind,
                name: row.name.clone(),
                template: row.template,
                deploy: row.deploy,
                after: row.after.clone(),
            });
        }
    }
}
fn desired(sync: &ResourceSync, gaps: &mut Vec<&'static str>) -> Option<Desired> {
    let files = &sync.info.remote_contents;
    if files.len() > 32 || files.iter().map(|f| f.contents.len()).sum::<usize>() > MAX_SOURCE {
        gap(gaps, "source_document_budget_exceeded");
        return None;
    }
    let kind = resource_syncs::summary_kind(&sync.config);
    if kind == "unconfigured" {
        gap(gaps, "source_unconfigured");
        return None;
    }
    if kind == "inline" && (files.len() != 1 || files[0].contents != sync.config.file_contents) {
        gap(gaps, "inline_cache_does_not_match_saved_definition");
        return None;
    }
    if matches!(kind, "git" | "linked_repo")
        && sync.info.pending_hash.as_deref().is_none_or(|s| {
            !(4..=64).contains(&s.len()) || !s.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        gap(gaps, "missing_git_source_revision");
        return None;
    }
    let mut desired = Desired {
        resources: vec![],
        variables: vec![],
        groups: vec![],
        deploy_requested: false,
    };
    if files.is_empty() && !sync.config.resource_path.is_empty() && kind != "inline" {
        gap(gaps, "no_observed_external_source_files");
        return None;
    }
    let mut identities = BTreeSet::new();
    let mut resource_names = BTreeSet::new();
    let mut variable_names = BTreeSet::new();
    let mut group_names = BTreeSet::new();
    for file in files {
        if !identities.insert((&file.resource_path, &file.path)) {
            gap(gaps, "duplicate_source_file_identity");
            return None;
        }
        let Ok(doc) = resource_syncs::parse_toml(&file.contents) else {
            gap(gaps, "source_document_unparseable_or_unsupported");
            return None;
        };
        macro_rules! extend {($(($field:ident,$kind:ident)),* $(,)?)=>{$(add_resources(&mut desired.resources,&doc.$field,Kind::$kind,&sync.config);)*};}
        extend!(
            (servers, Server),
            (swarms, Swarm),
            (stacks, Stack),
            (deployments, Deployment),
            (builds, Build),
            (repos, Repo),
            (builders, Builder),
            (alerters, Alerter),
            (procedures, Procedure),
            (actions, Action)
        );
        // Core deliberately excludes inline and unconfigured nested syncs.
        let sync_count = doc.resource_syncs.len();
        let syncs = doc
            .resource_syncs
            .into_iter()
            .filter(|s| {
                let c = &s.config;
                c.file_contents.as_ref().is_none_or(String::is_empty)
                    && (c.files_on_host == Some(true)
                        || c.repo.as_ref().is_some_and(|s| !s.is_empty())
                        || c.linked_repo.as_ref().is_some_and(|s| !s.is_empty()))
            })
            .collect::<Vec<_>>();
        if syncs.len() != sync_count {
            gap(
                gaps,
                "inline_or_unconfigured_nested_sync_definitions_ignored_by_core",
            );
        }
        add_resources(
            &mut desired.resources,
            &syncs,
            Kind::ResourceSync,
            &sync.config,
        );
        for v in doc.variables {
            if !name_ok(&v.name) || !variable_names.insert(v.name.clone()) {
                gap(gaps, "duplicate_or_unbounded_source_variable");
                return None;
            }
            desired.variables.push((v.name, v.is_secret));
        }
        for group in &doc.user_groups {
            if !name_ok(&group.name) || !group_names.insert(group.name.clone()) {
                gap(gaps, "duplicate_or_unbounded_source_group");
                return None;
            }
            desired.groups.push(group.name.clone());
        }
        if desired.resources.len() + desired.variables.len() + group_names.len() > 32 {
            gap(gaps, "source_entry_budget_exceeded");
            return None;
        }
    }
    for resource in &desired.resources {
        if !name_ok(&resource.name)
            || resource.after.len() > 32
            || !resource_names.insert((resource.kind, resource.name.clone()))
        {
            gap(gaps, "duplicate_or_unbounded_source_resource");
            return None;
        }
        if matches!(resource.kind, Kind::Stack | Kind::Deployment) && resource.deploy {
            desired.deploy_requested = true;
        }
    }
    Some(desired)
}
async fn metadata(
    client: &CoreClient,
    kind: Kind,
    selector: DetailInput,
) -> Result<Metadata, AppError> {
    let resolved = export::select(
        client,
        export::Target {
            kind,
            target: selector,
        },
    )
    .await?;
    let id = resolved.id.clone();
    macro_rules! get {
        ($request:ident,$field:ident) => {
            client.read_references($request { $field: id }).await?
        };
    }
    let raw = match kind {
        Kind::Server => get!(GetServer, server),
        Kind::Swarm => get!(GetSwarm, swarm),
        Kind::Stack => get!(GetStack, stack),
        Kind::Deployment => get!(GetDeployment, deployment),
        Kind::Build => get!(GetBuild, build),
        Kind::Repo => get!(GetRepo, repo),
        Kind::Builder => get!(GetBuilder, builder),
        Kind::Alerter => get!(GetAlerter, alerter),
        Kind::Procedure => get!(GetProcedure, procedure),
        Kind::Action => get!(GetAction, action),
        Kind::ResourceSync => get!(GetResourceSync, sync),
    };
    if raw["_id"]["$oid"].as_str() != Some(&resolved.id)
        || raw["name"].as_str() != Some(&resolved.name)
        || !raw["template"].is_boolean()
    {
        return Err(invalid());
    }
    Ok(Metadata {
        id: resolved.id,
        name: resolved.name,
        template: raw["template"].as_bool().expect("checked"),
    })
}
fn all_changes(snapshot: &Snapshot) -> Vec<Change> {
    snapshot
        .sync
        .info
        .resource_updates
        .iter()
        .take(MAX_CACHED)
        .filter_map(|r| {
            let (kind, id) = target(&r.target)?;
            Some(change(Some(kind), Some(id), &r.data, kind.header()).0)
        })
        .collect()
}
async fn selection(
    client: &CoreClient,
    input: Selection,
    snapshot: &Snapshot,
    desired: &Desired,
    rows: &[Change],
) -> Result<(ExecutionScope, Vec<Metadata>), AppError> {
    match input {
        Selection::WholeSync {} => Ok((ExecutionScope::WholeSync {}, vec![])),
        Selection::ResourceType { resource_type } => {
            Ok((ExecutionScope::ResourceType { resource_type }, vec![]))
        }
        Selection::Resources {
            resource_type,
            targets,
        } => {
            let mut resolved = Vec::new();
            let mut metadata_reads = Vec::new();
            let mut unique = BTreeSet::new();
            for input in targets {
                let created = rows
                    .iter()
                    .filter(|r| {
                        r.resource_type == Some(resource_type)
                            && r.action == ChangeKind::Create
                            && input
                                .name
                                .as_ref()
                                .is_some_and(|name| r.name.as_ref() == Some(name))
                    })
                    .collect::<Vec<_>>();
                let existing = rows
                    .iter()
                    .filter(|r| {
                        r.resource_type == Some(resource_type)
                            && r.action != ChangeKind::Create
                            && (input
                                .id
                                .as_ref()
                                .is_some_and(|id| r.id.as_ref() == Some(id))
                                || input
                                    .name
                                    .as_ref()
                                    .is_some_and(|name| r.name.as_ref() == Some(name)))
                    })
                    .collect::<Vec<_>>();
                if created.len() + existing.len() > 1 {
                    return Err(AppError::new(ErrorKind::AmbiguousTarget, OP, None));
                }
                let chosen = if let Some(row) = created.first() {
                    let name = row.name.clone().ok_or_else(invalid)?;
                    if valid_id(&name) {
                        return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
                    }
                    ResolvedTarget::Create { name }
                } else if let Some(row) = existing.first() {
                    ResolvedTarget::Existing {
                        id: row.id.clone().ok_or_else(invalid)?,
                        name: row.name.clone().ok_or_else(invalid)?,
                    }
                } else {
                    let m = metadata(client, resource_type, input).await?;
                    if !desired
                        .resources
                        .iter()
                        .any(|r| r.kind == resource_type && r.name == m.name)
                    {
                        return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
                    }
                    let target = ResolvedTarget::Existing {
                        id: m.id.clone(),
                        name: m.name.clone(),
                    };
                    metadata_reads.push(m);
                    target
                };
                let key = match &chosen {
                    ResolvedTarget::Existing { id, .. } => format!("id:{id}"),
                    ResolvedTarget::Create { name } => format!("name:{name}"),
                };
                if !unique.insert(key) {
                    return Err(AppError::arguments(
                        OP,
                        "Select each exact resource once; aliases resolving to the same identity are duplicates",
                    ));
                }
                resolved.push(chosen);
            }
            resolved.sort_by_cached_key(|r| serde_json::to_string(r).expect("selection"));
            let scope = ExecutionScope::Resources {
                resource_type,
                targets: resolved,
            };
            if snapshot.sync.config.include_resources {
                Ok((scope, metadata_reads))
            } else {
                Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None))
            }
        }
    }
}
fn raw_variable_changes(snapshot: &Snapshot) -> Vec<Change> {
    snapshot
        .sync
        .info
        .variable_updates
        .iter()
        .take(MAX_CACHED)
        .map(|d| change(None, None, d, "variable").0)
        .collect()
}
fn raw_group_changes(snapshot: &Snapshot) -> Vec<Change> {
    snapshot
        .sync
        .info
        .user_group_updates
        .iter()
        .take(MAX_CACHED)
        .map(|d| change(None, None, d, "user_group").0)
        .collect()
}
fn source_creation_name(kind: Kind, name: &str) -> String {
    match kind {
        Kind::Stack | Kind::Deployment | Kind::Build => to_docker_compatible_name(name),
        Kind::Repo => to_path_compatible_name(name),
        _ => to_general_name(name),
    }
}

pub(super) async fn prepare(
    client: &CoreClient,
    mut snapshot: Snapshot,
    selection_input: Selection,
) -> Result<(PendingOutput, Option<ApplyBinding>, Value), AppError> {
    let bounded_client = client.bounded_evidence();
    let mut gaps = snapshot.output.coverage_gaps.clone();
    let Some(desired) = desired(&snapshot.sync, &mut gaps) else {
        snapshot.output.coverage_gaps = gaps;
        return Ok((snapshot.output, None, Value::Null));
    };
    let mut rows = all_changes(&snapshot);
    let (scope, mut metadata_reads) =
        selection(&bounded_client, selection_input, &snapshot, &desired, &rows).await?;
    let whole = has_whole(&scope);
    if rows.iter().any(|r| {
        r.action != ChangeKind::Delete
            && !desired
                .resources
                .iter()
                .any(|s| Some(s.kind) == r.resource_type && Some(&s.name) == r.name.as_ref())
    }) {
        gap(&mut gaps, "cached_changes_not_covered_by_source_documents");
    }
    if rows.iter().any(|r| {
        r.action == ChangeKind::Delete
            && (!(snapshot.sync.config.managed || snapshot.sync.config.delete)
                || desired
                    .resources
                    .iter()
                    .any(|s| Some(s.kind) == r.resource_type && Some(&s.name) == r.name.as_ref()))
    }) {
        gap(
            &mut gaps,
            "cached_deletions_disagree_with_definition_or_source",
        );
    }
    if (!snapshot.sync.config.include_resources && !rows.is_empty())
        || (!snapshot.sync.config.include_variables
            && !snapshot.sync.info.variable_updates.is_empty())
        || (!snapshot.sync.config.include_user_groups
            && !snapshot.sync.info.user_group_updates.is_empty())
    {
        gap(&mut gaps, "cached_changes_disagree_with_inclusion_flags");
    }
    if !whole && !snapshot.sync.config.include_resources {
        gap(&mut gaps, "resource_selection_disabled_by_definition");
    }
    if desired.deploy_requested
        && (!whole
            || !snapshot.sync.config.include_resources
            || !snapshot.sync.config.match_tags.is_empty())
    {
        gap(
            &mut gaps,
            "deployment_execution_scope_differs_from_cached_view",
        );
    }
    if !desired.deploy_requested && !snapshot.sync.info.pending_deploys.is_empty() {
        gap(&mut gaps, "cached_deployments_without_source_intent");
    }
    if snapshot.output.settings.truncated || snapshot.output.settings.source.truncated {
        gap(&mut gaps, "definition_display_truncated");
    }
    if snapshot.sync.config.match_tags.iter().any(|s| valid_id(s)) {
        gap(
            &mut gaps,
            "objectid_shaped_tag_filter_is_not_faithfully_representable",
        );
    }
    let mut errors = Vec::new();
    let mut audit_metadata = Vec::new();
    if snapshot.sync.config.include_resources {
        for resource in &desired.resources {
            if !source_selected(&scope, resource) {
                continue;
            }
            let matches = rows
                .iter()
                .filter(|r| {
                    r.resource_type == Some(resource.kind)
                        && r.name.as_ref() == Some(&resource.name)
                })
                .collect::<Vec<_>>();
            if matches.len() > 1 {
                gap(&mut gaps, "duplicate_cached_resource_identity");
                continue;
            }
            if let Some(row) = matches.first() {
                if row.action == ChangeKind::Create
                    && (valid_id(&resource.name)
                        || source_creation_name(resource.kind, &resource.name) != resource.name)
                {
                    gap(
                        &mut gaps,
                        "source_creation_name_requires_normalization_or_is_an_objectid",
                    );
                }
                continue;
            }
            let existing = metadata_reads.iter().position(|m| m.name == resource.name);
            let m = if let Some(index) = existing {
                Some(metadata_reads.remove(index))
            } else {
                match metadata(
                    &bounded_client,
                    resource.kind,
                    DetailInput {
                        id: None,
                        name: Some(resource.name.clone()),
                    },
                )
                .await
                {
                    Ok(m) => Some(m),
                    Err(error) => {
                        gap(&mut gaps, "resource_metadata_not_authoritatively_observed");
                        if errors.len() < 8 {
                            errors.push(error);
                        }
                        None
                    }
                }
            };
            if let Some(m) = m {
                audit_metadata.push(json!({"resource_type":resource.kind,"id":m.id,"name":m.name,"template":m.template}));
                if m.template != resource.template {
                    rows.push(Change {
                        resource_type: Some(resource.kind),
                        id: Some(m.id),
                        name: Some(m.name),
                        action: ChangeKind::Update,
                        origin: "metadata_audit",
                        template_before: Some(m.template),
                        template_after: Some(resource.template),
                        secret_flag_before: None,
                        secret_flag_after: None,
                        details_omitted: true,
                    });
                }
            }
        }
    }
    let mut variables = raw_variable_changes(&snapshot);
    let groups = raw_group_changes(&snapshot);
    if variables.iter().any(|r| {
        r.action != ChangeKind::Delete
            && !desired
                .variables
                .iter()
                .any(|s| Some(&s.0) == r.name.as_ref())
    }) || groups.iter().any(|r| {
        r.action != ChangeKind::Delete && !desired.groups.iter().any(|s| Some(s) == r.name.as_ref())
    }) {
        gap(&mut gaps, "cached_changes_not_covered_by_source_documents");
    }
    if whole && snapshot.sync.config.include_variables && !desired.variables.is_empty() {
        match bounded_client.read_references(ListVariables {}).await {
            Ok(value) => {
                if let Some(items) = value.as_array()
                    && items.len() <= MAX_CACHED
                {
                    for (name, is_secret) in &desired.variables {
                        if variables.iter().any(|r| r.name.as_ref() == Some(name)) {
                            continue;
                        }
                        let matches = items
                            .iter()
                            .filter(|v| v["name"].as_str() == Some(name))
                            .collect::<Vec<_>>();
                        if matches.len() != 1 || !matches[0]["is_secret"].is_boolean() {
                            gap(
                                &mut gaps,
                                "variable_secret_flag_not_authoritatively_observed",
                            );
                            continue;
                        }
                        let before = matches[0]["is_secret"].as_bool().expect("checked");
                        audit_metadata.push(json!({"variable":name,"is_secret":before}));
                        if before != *is_secret {
                            variables.push(Change {
                                resource_type: None,
                                id: None,
                                name: Some(name.clone()),
                                action: ChangeKind::Update,
                                origin: "metadata_audit",
                                template_before: None,
                                template_after: None,
                                secret_flag_before: Some(before),
                                secret_flag_after: Some(*is_secret),
                                details_omitted: true,
                            });
                        }
                    }
                } else {
                    gap(&mut gaps, "variable_metadata_budget_or_shape_invalid");
                }
            }
            Err(error) => {
                gap(&mut gaps, "variable_metadata_not_observed");
                if errors.len() < 8 {
                    errors.push(error);
                }
            }
        }
    }
    let references =
        if !snapshot.sync.config.files_on_host && !snapshot.sync.config.linked_repo.is_empty() {
            let repo = &snapshot.sync.config.linked_repo;
            let selector = if valid_id(repo) {
                DetailInput {
                    id: Some(repo.clone()),
                    name: None,
                }
            } else {
                DetailInput {
                    id: None,
                    name: Some(repo.clone()),
                }
            };
            match inventory::resolve_repo(&bounded_client, selector).await {
                Ok((_, repo)) => {
                    if repo.template {
                        gap(&mut gaps, "linked_repo_is_template");
                    }
                    common::resource_baseline("repo", &repo)
                }
                Err(error) => {
                    gap(&mut gaps, "linked_repo_not_authoritatively_observed");
                    if errors.len() < 8 {
                        errors.push(error);
                    }
                    Value::Null
                }
            }
        } else {
            Value::Null
        };
    rows.retain(|r| {
        r.resource_type.is_some_and(|kind| {
            selected(
                &scope,
                kind,
                r.id.as_deref(),
                r.name.as_deref().unwrap_or_default(),
            )
        })
    });
    rows.sort_by_cached_key(|r| serde_json::to_string(r).expect("projection"));
    variables.sort_by_cached_key(|r| serde_json::to_string(r).expect("projection"));
    let mut unique = BTreeSet::new();
    for row in &rows {
        if !unique.insert((row.resource_type, row.id.clone(), row.name.clone())) {
            gap(&mut gaps, "duplicate_cached_resource_identity");
        }
    }
    let excluded = snapshot
        .sync
        .info
        .resource_updates
        .len()
        .saturating_sub(rows.iter().filter(|r| r.origin == "core_cache").count());
    let resource_total = rows.len();
    let mut resource_section = section(rows, true, resource_total);
    resource_section.excluded_by_selection = excluded;
    let variable_count = if whole { variables.len() } else { 0 };
    let group_count = if whole { groups.len() } else { 0 };
    let mut variable_section =
        section(if whole { variables } else { vec![] }, true, variable_count);
    let mut group_section = section(if whole { groups } else { vec![] }, true, group_count);
    if !whole {
        variable_section.excluded_by_selection = snapshot.sync.info.variable_updates.len();
        group_section.excluded_by_selection = snapshot.sync.info.user_group_updates.len();
    }
    if resource_section.truncated
        || variable_section.truncated
        || group_section.truncated
        || snapshot.output.pending_deployments.truncated
    {
        gap(&mut gaps, "selected_display_truncated");
    }
    if !snapshot.complete {
        gap(&mut gaps, "authoritative_cache_incomplete");
    }
    if snapshot.sync.template {
        gap(&mut gaps, "sync_is_template");
    }
    // Source identities/contents and full cached TOML remain private and exact.
    audit_metadata.sort_by_cached_key(Value::to_string);
    let baseline = canonical(&snapshot, references, Value::Array(audit_metadata));
    let mut output = snapshot.output;
    output.resource_changes = resource_section;
    output.variable_changes = variable_section;
    output.user_group_changes = group_section;
    output.selection = Some(scope.clone());
    output.observation_errors = errors;
    output.coverage_gaps = gaps;
    output.variables_in_review_scope = Some(whole && snapshot.sync.config.include_variables);
    output.user_groups_in_review_scope = Some(whole && snapshot.sync.config.include_user_groups);
    output.apply_coverage_complete = output.coverage_gaps.is_empty();
    let pending = output
        .resource_changes
        .reported_total
        .is_some_and(|n| n > 0)
        || output
            .variable_changes
            .reported_total
            .is_some_and(|n| n > 0)
        || output
            .user_group_changes
            .reported_total
            .is_some_and(|n| n > 0)
        || output
            .pending_deployments
            .reported_total
            .is_some_and(|n| n > 0);
    output.reviewed_status = Some(if output.cached_status == "errors" {
        "errors"
    } else if !output.apply_coverage_complete {
        "incomplete"
    } else if pending {
        "pending"
    } else {
        "clean"
    });
    let binding = ApplyBinding {
        sync_id: snapshot.sync.id,
        selection: scope,
    };
    Ok((output, Some(binding), baseline))
}
