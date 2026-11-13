//! Exact tag identities and explicit assignment sets through the shared preview contract.
use crate::{
    client::CoreClient,
    configuration::{self as common, Preview},
    deletion::{self, Coverage, ResourceKind, Target, object_id, raw_id},
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput},
};
use komodo_client::{
    api::{
        read::{GetDeployment, GetServer, GetStack, ListTags},
        write::{CreateTag, DeleteTag, RenameTag, UpdateResourceMeta},
    },
    entities::{ResourceTarget, tag::TagColor},
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    collections::{BTreeMap, BTreeSet},
    time::{Duration, Instant},
};

mod assignments;
pub use assignments::Assignment;

fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, "tags", None)
}
fn args(message: &'static str) -> AppError {
    AppError::arguments("tags", message)
}
fn stale() -> AppError {
    AppError {
        correction: Some(
            "The tag identity, assignment set or visible deletion effects changed. Prepare a new preview; no write was submitted.",
        ),
        ..AppError::new(ErrorKind::StalePreview, "tags", None)
    }
}
fn name_valid(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
fn new_name(client: &CoreClient, s: &str) -> Result<(), AppError> {
    common::validate_payload(client, &s)?;
    if !name_valid(s) || object_id(s) {
        return Err(args(
            "Use an exact nonempty tag name of at most 128 bytes, without control characters or an ObjectId-shaped name; names retain their case and whitespace",
        ));
    }
    Ok(())
}
fn selector(s: &DetailInput) -> Result<Value, AppError> {
    match (&s.id, &s.name) {
        (Some(id), None) if object_id(id) => Ok(json!({"_id":{"$oid":id.to_ascii_lowercase()}})),
        (None, Some(name)) if name_valid(name) => Ok(json!({"name":name})),
        _ => Err(args(
            "Select exactly one stable 24-hexadecimal tag id or exact case-sensitive tag name",
        )),
    }
}
fn selector_matches(s: &DetailInput, v: &Value) -> bool {
    s.id.as_ref().is_some_and(|id| {
        v["id"]
            .as_str()
            .is_some_and(|found| found.eq_ignore_ascii_case(id))
    }) || s.name.as_ref().is_some_and(|name| v["name"] == *name)
}
fn tag_signature(v: &Value) -> Result<Value, AppError> {
    let id = raw_id(v)?;
    let name = v["name"]
        .as_str()
        .filter(|s| !s.trim().is_empty() && s.len() <= 4096)
        .ok_or_else(invalid)?;
    let owner = v["owner"]
        .as_str()
        .filter(|s| s.len() <= 128)
        .ok_or_else(invalid)?;
    let color = v.get("color").ok_or_else(invalid)?;
    serde_json::from_value::<TagColor>(color.clone()).map_err(|_| invalid())?;
    Ok(json!({"id":id,"name":name,"owner":owner,"color":color}))
}
async fn tags_query(client: &CoreClient, query: Value, max: usize) -> Result<Vec<Value>, AppError> {
    let raw = client
        .read_references(ListTags {
            query: Some(serde_json::from_value(query).map_err(|_| invalid())?),
        })
        .await?;
    let list = raw
        .as_array()
        .filter(|a| a.len() <= max)
        .ok_or_else(invalid)?;
    let mut seen = BTreeSet::new();
    let mut result = vec![];
    for value in list {
        let tag = tag_signature(value)?;
        if !seen.insert(tag["id"].as_str().expect("id").to_owned()) {
            return Err(invalid());
        }
        result.push(tag)
    }
    result.sort_by_key(|v| v["id"].as_str().expect("id").to_owned());
    Ok(result)
}
async fn lookup(client: &CoreClient, s: &DetailInput) -> Result<Option<Value>, AppError> {
    let mut tags = tags_query(client, selector(s)?, 1).await?;
    if tags.iter().any(|v| !selector_matches(s, v)) {
        return Err(invalid());
    }
    Ok(tags.pop())
}
async fn resolve(client: &CoreClient, s: &DetailInput) -> Result<Value, AppError> {
    lookup(client, s)
        .await?
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, "ResolveTag", None))
}
fn id_selector(id: &str) -> DetailInput {
    DetailInput {
        id: Some(id.into()),
        name: None,
    }
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct TagView {
    pub id: Option<String>,
    pub name: String,
    pub color: String,
}
fn view(client: &CoreClient, v: &Value) -> TagView {
    TagView {
        id: v["id"].as_str().map(str::to_owned),
        name: common::bounded_text(client, v["name"].as_str().expect("tag name"), 256),
        color: v["color"].as_str().expect("tag color").into(),
    }
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct AssignedTag {
    pub id: String,
    pub name: Option<String>,
    pub color: Option<String>,
    pub error: Option<AppError>,
}
fn tag_set(
    client: &CoreClient,
    ids: &[String],
    tags: &BTreeMap<String, Value>,
) -> Vec<AssignedTag> {
    ids.iter()
        .map(|id| match tags.get(id) {
            Some(t) => AssignedTag {
                id: id.clone(),
                name: Some(common::bounded_text(
                    client,
                    t["name"].as_str().expect("tag"),
                    256,
                )),
                color: t["color"].as_str().map(str::to_owned),
                error: None,
            },
            None => AssignedTag {
                id: id.clone(),
                name: None,
                color: None,
                error: Some(AppError::new(
                    ErrorKind::NotFound,
                    "ResolveAssignedTag",
                    None,
                )),
            },
        })
        .collect()
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct ResourceView {
    pub kind: ResourceKind,
    pub id: String,
    pub name: String,
}
#[derive(Clone, Copy, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Preview,
    NoOp,
    Acknowledged,
    SubmissionUnknown,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct TagResult {
    pub operation: &'static str,
    pub outcome: Outcome,
    pub preview_id: Option<String>,
    pub writes_submitted: u8,
    pub change_verified: bool,
    pub tag_before: Option<TagView>,
    pub tag_after: Option<TagView>,
    pub resource: Option<ResourceView>,
    pub before_tags: Vec<AssignedTag>,
    pub after_tags: Vec<AssignedTag>,
    pub observed_tags: Option<Vec<AssignedTag>>,
    pub tag_record_observation: Option<String>,
    pub tag_removal_reported_by_core: bool,
    pub visible_assignments: Vec<Assignment>,
    pub coverage: Vec<Coverage>,
    pub assignment_cleanup_fully_verified: bool,
    pub effects: Vec<&'static str>,
    pub evidence_gaps: Vec<&'static str>,
    pub errors: Vec<AppError>,
}
fn result(operation: &'static str) -> TagResult {
    TagResult {
        operation,
        outcome: Outcome::Preview,
        preview_id: None,
        writes_submitted: 0,
        change_verified: false,
        tag_before: None,
        tag_after: None,
        resource: None,
        before_tags: vec![],
        after_tags: vec![],
        observed_tags: None,
        tag_record_observation: None,
        tag_removal_reported_by_core: false,
        visible_assignments: vec![],
        coverage: vec![],
        assignment_cleanup_fully_verified: false,
        effects: vec![
            "Preview submits zero writes. The shared process-local store retains at most sixteen ten-minute single-use previews, bound to operation, exact identity, proposed change and private relevant baseline.",
            "Core remains the permission authority; the same authenticated transport has bounded deadlines/bodies, no redirects, automatic mutation retry or credential fallback.",
            "Core has no conditional write or invocation idempotency. Another caller can race after final revalidation. Lost, invalid or mismatched replies retain submission_unknown even when later reads observe the intended state.",
        ],
        evidence_gaps: vec![],
        errors: vec![],
    }
}

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub name: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyCreate {
    pub name: String,
    pub preview_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct RenameInput {
    pub tag: DetailInput,
    pub name: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyRename {
    pub tag: DetailInput,
    pub name: String,
    pub preview_id: String,
}
#[derive(Clone, Copy, Default, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum AssignmentMode {
    #[default]
    Replace,
    Add,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct AssignInput {
    pub target: Target,
    #[serde(default)]
    pub mode: AssignmentMode,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "common::present"
    )]
    #[schemars(with = "Vec<DetailInput>")]
    pub tags: Option<Vec<DetailInput>>,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyAssign {
    pub target: Target,
    #[serde(default)]
    pub mode: AssignmentMode,
    #[serde(default, deserialize_with = "common::present")]
    #[schemars(with = "Vec<DetailInput>")]
    pub tags: Option<Vec<DetailInput>>,
    pub preview_id: String,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DeleteInput {
    pub tag: DetailInput,
    pub action: deletion::Action,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyDelete {
    pub tag: DetailInput,
    pub action: deletion::Action,
    pub preview_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FindInput {
    pub tag: DetailInput,
    pub kind: ResourceKind,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "page_size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u64,
}
fn page_size() -> u64 {
    20
}
#[derive(Serialize, JsonSchema)]
pub struct FindResult {
    pub tag: TagView,
    pub resources: Vec<Assignment>,
    pub page: u64,
    pub page_size: u64,
    pub has_more: bool,
    pub next_page: Option<u64>,
    pub hidden_records_possible: bool,
    pub evidence_gaps: Vec<&'static str>,
}

enum Action {
    Create(CreateTag),
    Rename(RenameTag),
    Assign(UpdateResourceMeta),
    Delete(DeleteTag),
}
impl Action {
    fn operation(&self) -> &'static str {
        match self {
            Self::Create(_) => "tag_create",
            Self::Rename(_) => "tag_rename",
            Self::Assign(_) => "resource_tags",
            Self::Delete(_) => "tag_delete",
        }
    }
    fn parameters(&self) -> Value {
        match self {
            Self::Create(r) => serde_json::to_value(r),
            Self::Rename(r) => serde_json::to_value(r),
            Self::Assign(r) => serde_json::to_value(r),
            Self::Delete(r) => serde_json::to_value(r),
        }
        .expect("typed tag request")
    }
}
struct Prepared {
    baseline: Value,
    action: Action,
    result: TagResult,
    no_op: bool,
}
impl Prepared {
    fn parameters(&self) -> Value {
        json!({"operation":self.action.operation(),"params":self.action.parameters(),"no_op":self.no_op})
    }
}
async fn bounded<F, T>(future: F) -> Result<T, AppError>
where
    F: std::future::Future<Output = Result<T, AppError>>,
{
    tokio::time::timeout(Duration::from_secs(15),future).await.unwrap_or_else(|_|Err(AppError{correction:Some("Tag preflight exceeded its budget. No write was submitted; inspect the target and prepare a new preview."),..AppError::new(ErrorKind::Timeout,"TagPreflight",None)}))
}
fn store(client: &CoreClient, input: &impl Serialize, p: Prepared) -> Result<TagResult, AppError> {
    if p.baseline.to_string().len() > 256 * 1024 {
        return Err(args(
            "The complete private tag-effect snapshot exceeds 256 KiB; inspect this larger change directly in Core",
        ));
    }
    let parameters = p.parameters();
    client.previews().insert_tags(Preview {
        created: Instant::now(),
        input: common::bound_input(p.action.operation(), input),
        baseline: p.baseline,
        prepared: parameters,
        result: p.result,
    })
}
fn take(
    client: &CoreClient,
    token: &str,
    operation: &'static str,
    input: &impl Serialize,
) -> Result<Preview<TagResult>, AppError> {
    if token.len() > 128 {
        return Err(AppError::new(ErrorKind::InvalidPreview, operation, None));
    }
    client
        .previews()
        .take_tags(token, &common::bound_input(operation, input))
}
fn compare(entry: &Preview<TagResult>, p: &Prepared) -> Result<(), AppError> {
    if p.baseline.to_string().len() > 256 * 1024
        || p.baseline != entry.baseline
        || p.parameters() != entry.prepared
    {
        return Err(stale());
    }
    Ok(())
}

async fn prepare_create(client: &CoreClient, input: &CreateInput) -> Result<Prepared, AppError> {
    new_name(client, &input.name)?;
    let found = lookup(
        client,
        &DetailInput {
            id: None,
            name: Some(input.name.clone()),
        },
    )
    .await?;
    if found.is_some() {
        return Err(args(
            "That exact tag name already exists; inspect it and use its stable identity",
        ));
    }
    let mut r = result("tag_create");
    r.tag_after = Some(view(client, &json!({"name":input.name,"color":"Slate"})));
    r.effects.push("Create one exact case-sensitive tag using Core's default Slate color. Core assigns ownership to the authenticated caller and enforces its non-admin creation policy. No resource assignment or workload execution is submitted.");
    Ok(Prepared {
        baseline: json!({"name_absent":input.name}),
        action: Action::Create(CreateTag {
            name: input.name.clone(),
            color: None,
        }),
        result: r,
        no_op: false,
    })
}
async fn prepare_rename(client: &CoreClient, input: &RenameInput) -> Result<Prepared, AppError> {
    new_name(client, &input.name)?;
    let tag = resolve(client, &input.tag).await?;
    let occupied = lookup(
        client,
        &DetailInput {
            id: None,
            name: Some(input.name.clone()),
        },
    )
    .await?;
    if occupied.as_ref().is_some_and(|t| t["id"] != tag["id"]) {
        return Err(args("The requested exact tag name belongs to another tag"));
    }
    let no_op = tag["name"] == input.name;
    let mut after = tag.clone();
    after["name"] = json!(input.name);
    let mut r = result("tag_rename");
    r.tag_before = Some(view(client, &tag));
    r.tag_after = Some(view(client, &after));
    if no_op {
        r.outcome = Outcome::NoOp
    }
    r.effects.push("Rename this exact stable tag identity only; preserve its color, owner and resource assignment IDs. Core requires the tag owner or an admin. Discovery by the new name resolves the same ID; name uniqueness and final-read/write races remain Core-controlled.");
    Ok(Prepared {
        baseline: json!({"tag":tag,"occupied":occupied}),
        action: Action::Rename(RenameTag {
            id: tag["id"].as_str().expect("id").into(),
            name: input.name.clone(),
        }),
        result: r,
        no_op,
    })
}
fn ids(v: &Value) -> Result<Vec<String>, AppError> {
    let items = v.as_array().filter(|a| a.len() <= 64).ok_or_else(invalid)?;
    let mut result = BTreeSet::new();
    for id in items {
        let id = id
            .as_str()
            .filter(|s| object_id(s))
            .ok_or_else(invalid)?
            .to_ascii_lowercase();
        if !result.insert(id) {
            return Err(invalid());
        }
    }
    Ok(result.into_iter().collect())
}
fn resource_signature(mut raw: Value) -> Result<Value, AppError> {
    let id = raw_id(&raw)?;
    if raw["name"].as_str().is_none_or(str::is_empty)
        || !raw["config"].is_object()
        || !raw["template"].is_boolean()
        || !raw["description"].is_string()
    {
        return Err(invalid());
    }
    ids(&raw["tags"])?;
    let object = raw.as_object_mut().ok_or_else(invalid)?;
    for key in ["_id", "info", "updated_at"] {
        object.remove(key);
    }
    object.insert("id".into(), json!(id));
    Ok(raw)
}
async fn raw_resource(client: &CoreClient, t: &Target) -> Result<Value, AppError> {
    match t.kind {
        ResourceKind::Stack => {
            client
                .read_references(GetStack {
                    stack: t.id.clone(),
                })
                .await
        }
        ResourceKind::Deployment => {
            client
                .read_references(GetDeployment {
                    deployment: t.id.clone(),
                })
                .await
        }
        ResourceKind::Server => {
            client
                .read_references(GetServer {
                    server: t.id.clone(),
                })
                .await
        }
    }
}
async fn selected_resource(client: &CoreClient, t: &Target) -> Result<Value, AppError> {
    let s = DetailInput {
        id: Some(t.id.clone()),
        name: None,
    };
    let typed = match t.kind {
        ResourceKind::Stack => resource_signature(
            serde_json::to_value(inventory::resolve_stack(client, s).await?.1).expect("resource"),
        )?,
        ResourceKind::Deployment => resource_signature(
            serde_json::to_value(inventory::resolve_deployment(client, s).await?.1)
                .expect("resource"),
        )?,
        ResourceKind::Server => resource_signature(
            serde_json::to_value(inventory::resolve_server(client, s).await?.1).expect("resource"),
        )?,
    };
    let raw = resource_signature(raw_resource(client, t).await?)?;
    if typed != raw {
        return Err(stale());
    }
    Ok(raw)
}
async fn prepare_assign(client: &CoreClient, input: &AssignInput) -> Result<Prepared, AppError> {
    let target = input.target.clone().validate()?;
    if input.tags.as_ref().is_some_and(|t| t.len() > 32) {
        return Err(args(
            "Assign at most 32 requested tags and retain at most 64 total tag identities",
        ));
    }
    for tag in input.tags.as_deref().unwrap_or(&[]) {
        selector(tag)?;
    }
    let resource = selected_resource(client, &target).await?;
    let before = ids(&resource["tags"])?;
    let mut queries = before
        .iter()
        .map(|id| selector(&id_selector(id)))
        .collect::<Result<Vec<_>, _>>()?;
    let requested = input.tags.as_deref().unwrap_or(&[]);
    for s in requested {
        queries.push(selector(s)?)
    }
    let tags = if queries.is_empty() {
        vec![]
    } else {
        tags_query(
            client,
            json!({"$or":queries}),
            before.len() + requested.len(),
        )
        .await?
    };
    if tags.iter().any(|v| {
        !before.iter().any(|id| v["id"] == *id) && !requested.iter().any(|s| selector_matches(s, v))
    }) {
        return Err(invalid());
    }
    let map = tags
        .iter()
        .map(|v| (v["id"].as_str().expect("id").to_owned(), v.clone()))
        .collect::<BTreeMap<_, _>>();
    let mut desired = BTreeSet::new();
    if input.tags.is_none() || matches!(input.mode, AssignmentMode::Add) {
        desired.extend(before.iter().cloned());
    }
    for s in requested {
        let tag = tags
            .iter()
            .find(|v| selector_matches(s, v))
            .ok_or_else(|| AppError::new(ErrorKind::NotFound, "ResolveTag", None))?;
        desired.insert(tag["id"].as_str().expect("id").into());
    }
    if desired.len() > 64 {
        return Err(args("The exact resulting assignment set exceeds 64 tags"));
    }
    let after = desired.into_iter().collect::<Vec<_>>();
    let no_op = after == before;
    if !no_op && after.iter().any(|id| !map.contains_key(id)) {
        return Err(args(
            "An existing assignment cannot be resolved to a tag. Inspect it in Core before adding; unresolved IDs could be silently dropped upstream",
        ));
    }
    let mut r = result("resource_tags");
    r.resource = Some(ResourceView {
        kind: target.kind,
        id: target.id.clone(),
        name: common::bounded_text(client, resource["name"].as_str().expect("name"), 256),
    });
    r.before_tags = tag_set(client, &before, &map);
    r.after_tags = tag_set(client, &after, &map);
    if no_op {
        r.outcome = Outcome::NoOp
    }
    r.effects.extend(["Omitted tags preserve the assignment set with zero writes. Replace with [] deliberately clears it; add unions resolved IDs with existing assignments and preserves unrelated tags. Existing order is treated as set semantics.","Only target and the exact stable tag-ID set are sent through typed UpdateResourceMeta. Unrelated description, template and configuration are preserved. Core requires Write on the resource; tag ownership is not required for assignment.","Core can attempt implicit creation for unresolved names and silently drop failed tag resolutions. MCP sends only resolved stable IDs and rechecks them; a concurrent tag deletion or final read-modify-write race can still change Core's result. Exact authoritative read-back is reported separately without retry."]);
    let core_target = match target.kind {
        ResourceKind::Stack => ResourceTarget::Stack(target.id.clone()),
        ResourceKind::Deployment => ResourceTarget::Deployment(target.id.clone()),
        ResourceKind::Server => ResourceTarget::Server(target.id.clone()),
    };
    Ok(Prepared {
        baseline: json!({"resource":resource,"tags":tags}),
        action: Action::Assign(UpdateResourceMeta {
            target: core_target,
            description: None,
            template: None,
            tags: Some(after),
        }),
        result: r,
        no_op,
    })
}
async fn prepare_delete(client: &CoreClient, input: &DeleteInput) -> Result<Prepared, AppError> {
    let tag = resolve(client, &input.tag).await?;
    let id = tag["id"].as_str().expect("id");
    let scan = assignments::scan(client, id).await?;
    let mut r = result("tag_delete");
    r.tag_before = Some(view(client, &tag));
    r.visible_assignments = scan.assignments.into_iter().take(32).collect();
    r.coverage = scan.coverage;
    r.effects.extend(["Both independent local grants are required for deliberate tag deletion, even when no visible assignment is found. Core requires tag ownership or admin; this can remove assignments on resources the caller cannot see or edit.","Core first removes this exact tag ID across Server, Stack, Deployment, Build, Repo, Procedure, Action, ResourceSync, Builder and Alerter records, then deletes the tag. A failure can leave partially removed assignments while the tag remains. No separate workload execution or repair request is submitted.","Pinned Core 2.3.2 omits Swarm records from tag-deletion cleanup. Existing Swarm references can remain dangling; each visible after-state is reported separately. Tag record absence is not proof that all assignments were removed."]);
    r.evidence_gaps.extend(["Visible assignment scans are bounded to two positive pages of twenty records per family; hidden resources, additional pages and concurrent assignments remain unknown. At most thirty-two assignments are displayed; all retained visible effects remain privately bound.","Tag deletion has no invocation update ID, conditional delete or idempotency key. Cross-collection cleanup is not atomic. No global assignment-cleanup guarantee follows from an acknowledgement or missing tag."]);
    Ok(Prepared {
        baseline: json!({"tag":tag,"assignments":scan.snapshot}),
        action: Action::Delete(DeleteTag { id: id.into() }),
        result: r,
        no_op: false,
    })
}

pub async fn preview_create(
    client: &CoreClient,
    input: CreateInput,
) -> Result<TagResult, AppError> {
    common::require_edit(client)?;
    common::validate_payload(client, &input)?;
    let c = client.bounded_evidence();
    store(client, &input, bounded(prepare_create(&c, &input)).await?)
}
pub async fn preview_rename(
    client: &CoreClient,
    input: RenameInput,
) -> Result<TagResult, AppError> {
    common::require_edit(client)?;
    common::validate_payload(client, &input)?;
    selector(&input.tag)?;
    let c = client.bounded_evidence();
    store(client, &input, bounded(prepare_rename(&c, &input)).await?)
}
pub async fn preview_assign(
    client: &CoreClient,
    input: AssignInput,
) -> Result<TagResult, AppError> {
    common::require_edit(client)?;
    common::validate_payload(client, &input)?;
    input.target.clone().validate()?;
    let c = client.bounded_evidence();
    store(client, &input, bounded(prepare_assign(&c, &input)).await?)
}
pub async fn preview_delete(
    client: &CoreClient,
    input: DeleteInput,
) -> Result<TagResult, AppError> {
    common::require_edit(client)?;
    common::validate_payload(client, &input)?;
    selector(&input.tag)?;
    let c = client.bounded_evidence();
    store(client, &input, bounded(prepare_delete(&c, &input)).await?)
}

fn expected_tag(p: &Prepared) -> Option<Value> {
    match &p.action {
        Action::Create(r) => Some(json!({"name":r.name,"color":"Slate"})),
        Action::Rename(r) => {
            let mut tag = p.baseline["tag"].clone();
            tag["name"] = json!(r.name);
            Some(tag)
        }
        Action::Delete(_) => Some(p.baseline["tag"].clone()),
        Action::Assign(_) => None,
    }
}
async fn observe(
    client: &CoreClient,
    p: &Prepared,
    ack: Option<&Value>,
    r: &mut TagResult,
) -> Result<(), AppError> {
    match &p.action {
        Action::Create(request) => {
            let observed = lookup(
                client,
                &DetailInput {
                    id: None,
                    name: Some(request.name.clone()),
                },
            )
            .await?;
            if let Some(tag) = observed {
                r.tag_after = Some(view(client, &tag));
                r.change_verified = ack.is_some_and(|a| *a == tag);
                if ack.is_some() && !r.change_verified {
                    r.outcome = Outcome::SubmissionUnknown;
                    r.errors.push(invalid());
                }
                r.tag_record_observation = Some(
                    if r.change_verified {
                        "matching_tag_present_at_exact_read"
                    } else {
                        "matching_name_present_invocation_not_proven"
                    }
                    .into(),
                );
            } else {
                r.tag_record_observation = Some("tag_absent_at_exact_name_read".into())
            }
        }
        Action::Rename(request) => {
            let observed = lookup(client, &id_selector(&request.id)).await?;
            if let Some(tag) = observed {
                r.tag_after = Some(view(client, &tag));
                r.change_verified = Some(tag.clone()) == expected_tag(p);
                r.tag_record_observation = Some("tag_present_at_exact_id_read".into());
            } else {
                r.tag_record_observation = Some("tag_absent_at_exact_id_read".into())
            }
        }
        Action::Assign(request) => {
            let target = r.resource.as_ref().expect("resource");
            let raw = resource_signature(
                raw_resource(
                    client,
                    &Target {
                        kind: target.kind,
                        id: target.id.clone(),
                    },
                )
                .await?,
            )?;
            if raw["id"] != target.id {
                return Err(invalid());
            }
            let observed = ids(&raw["tags"])?;
            let map = p.baseline["tags"]
                .as_array()
                .expect("tag snapshot")
                .iter()
                .map(|t| (t["id"].as_str().expect("id").to_owned(), t.clone()))
                .collect();
            r.observed_tags = Some(tag_set(client, &observed, &map));
            let mut expected = p.baseline["resource"].clone();
            expected["tags"] = json!(request.tags.as_ref().expect("exact tags"));
            let mut canonical = raw;
            canonical["tags"] = json!(observed);
            let mut before = expected;
            before["tags"] = json!(ids(&before["tags"])?);
            r.change_verified = canonical == before;
            if !r.change_verified {
                r.errors.push(AppError{message:"Read-back differs from the exact requested tag set or preserved resource metadata. Inspect this resource; no retry was attempted.",..AppError::new(ErrorKind::InvalidResponse,"TagAssignmentReadback",None)});
            }
        }
        Action::Delete(request) => {
            let observed = lookup(client, &id_selector(&request.id)).await?;
            r.tag_record_observation = Some(
                if observed.is_none() {
                    "tag_absent_at_exact_id_read"
                } else {
                    "tag_still_present_at_exact_id_read"
                }
                .into(),
            );
            r.change_verified = observed.is_none();
            assignments::observe(client, &request.id, &mut r.visible_assignments).await;
        }
    }
    Ok(())
}
async fn apply_prepared(
    client: &CoreClient,
    entry: Preview<TagResult>,
    p: Prepared,
) -> Result<TagResult, AppError> {
    compare(&entry, &p)?;
    let mut r = entry.result;
    r.preview_id = None;
    if p.no_op {
        r.outcome = Outcome::NoOp;
        r.change_verified = true;
        r.observed_tags = Some(r.before_tags.clone());
        return Ok(r);
    }
    r.writes_submitted = 1;
    let reply = match &p.action {
        Action::Create(q) => client.tag_write(q.clone()).await,
        Action::Rename(q) => client.tag_write(q.clone()).await,
        Action::Assign(q) => client.tag_write(q.clone()).await,
        Action::Delete(q) => client.delete(q.clone()).await,
    };
    let ack = reply.and_then(|raw| match &p.action {
        Action::Assign(_) => {
            if raw == json!({}) {
                Ok(None)
            } else {
                Err(invalid())
            }
        }
        Action::Create(_) => {
            let tag = tag_signature(&raw)?;
            let desired = expected_tag(&p).expect("create");
            if tag["name"] == desired["name"] && tag["color"] == desired["color"] {
                Ok(Some(tag))
            } else {
                Err(invalid())
            }
        }
        _ => {
            let tag = tag_signature(&raw)?;
            if Some(tag.clone()) == expected_tag(&p) {
                Ok(Some(tag))
            } else {
                Err(invalid())
            }
        }
    });
    let ack_tag = match ack {
        Ok(tag) => {
            r.outcome = Outcome::Acknowledged;
            r.tag_removal_reported_by_core = matches!(p.action, Action::Delete(_));
            tag
        }
        Err(e) => {
            r.outcome = Outcome::SubmissionUnknown;
            r.errors.push(e);
            None
        }
    };
    let c = client.bounded_evidence();
    match tokio::time::timeout(
        Duration::from_secs(10),
        observe(&c, &p, ack_tag.as_ref(), &mut r),
    )
    .await
    {
        Ok(Ok(())) => {}
        Ok(Err(e)) => r.errors.push(e),
        Err(_) => {
            r.errors
                .push(AppError::new(ErrorKind::Timeout, "TagObservation", None));
            r.evidence_gaps.push("Post-attempt read-back exceeded its observation budget; unchecked tag/assignment effects remain unknown. No further write was submitted.");
        }
    }
    if !r.change_verified {
        r.evidence_gaps.push("The intended change was not fully corroborated at read-back; inspect the exact tag/resource and do not automatically repeat the mutation.");
    }
    if r.tag_record_observation.is_none() && !matches!(p.action, Action::Assign(_)) {
        r.tag_record_observation = Some("observation_unavailable".into());
    }
    for assignment in &mut r.visible_assignments {
        if assignment.observation.is_none() {
            assignment.observation = Some("post_attempt_not_checked_within_budget".into());
        }
    }
    Ok(r)
}
pub async fn apply_create(client: &CoreClient, input: ApplyCreate) -> Result<TagResult, AppError> {
    common::require_edit(client)?;
    let canonical = CreateInput { name: input.name };
    common::validate_payload(client, &canonical)?;
    new_name(client, &canonical.name)?;
    let entry = take(client, &input.preview_id, "tag_create", &canonical)?;
    let c = client.bounded_evidence();
    let p = bounded(prepare_create(&c, &canonical)).await?;
    apply_prepared(client, entry, p).await
}
pub async fn apply_rename(client: &CoreClient, input: ApplyRename) -> Result<TagResult, AppError> {
    common::require_edit(client)?;
    let canonical = RenameInput {
        tag: input.tag,
        name: input.name,
    };
    common::validate_payload(client, &canonical)?;
    new_name(client, &canonical.name)?;
    selector(&canonical.tag)?;
    let entry = take(client, &input.preview_id, "tag_rename", &canonical)?;
    let c = client.bounded_evidence();
    let p = bounded(prepare_rename(&c, &canonical)).await?;
    apply_prepared(client, entry, p).await
}
pub async fn apply_assign(client: &CoreClient, input: ApplyAssign) -> Result<TagResult, AppError> {
    common::require_edit(client)?;
    let canonical = AssignInput {
        target: input.target,
        mode: input.mode,
        tags: input.tags,
    };
    common::validate_payload(client, &canonical)?;
    canonical.target.clone().validate()?;
    let entry = take(client, &input.preview_id, "resource_tags", &canonical)?;
    let c = client.bounded_evidence();
    let p = bounded(prepare_assign(&c, &canonical)).await?;
    apply_prepared(client, entry, p).await
}
pub async fn apply_delete(client: &CoreClient, input: ApplyDelete) -> Result<TagResult, AppError> {
    deletion::require_apply(client)?;
    let canonical = DeleteInput {
        tag: input.tag,
        action: input.action,
    };
    common::validate_payload(client, &canonical)?;
    selector(&canonical.tag)?;
    let entry = take(client, &input.preview_id, "tag_delete", &canonical)?;
    let c = client.bounded_evidence();
    let p = bounded(prepare_delete(&c, &canonical)).await?;
    apply_prepared(client, entry, p).await
}

pub async fn find(client: &CoreClient, input: FindInput) -> Result<FindResult, AppError> {
    selector(&input.tag)?;
    if !(1..=100).contains(&input.page_size)
        || input
            .page
            .checked_add(1)
            .and_then(|p| p.checked_mul(input.page_size))
            .and_then(|n| usize::try_from(n).ok())
            .is_none()
    {
        return Err(args(
            "Use a positive page size from 1 to 100 and bounded current/next offsets",
        ));
    }
    let c = client.bounded_evidence();
    bounded(async{let tag=resolve(&c,&input.tag).await?;let id=tag["id"].as_str().expect("id");let resources=assignments::find_page(&c,input.kind,id,input.page,input.page_size).await?;let has_more=resources.len()==input.page_size as usize&&!assignments::find_page(&c,input.kind,id,input.page+1,input.page_size).await?.is_empty();Ok(FindResult{tag:view(client,&tag),resources,page:input.page,page_size:input.page_size,has_more,next_page:has_more.then_some(input.page+1),hidden_records_possible:true,evidence_gaps:vec!["This exact-tag query covers visible records only and is not a snapshot; hidden resources and concurrent changes remain possible."]})}).await
}
