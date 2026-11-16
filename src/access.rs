//! Scoped administration. Raw authentication and grant baselines remain private.
use crate::{
    client::CoreClient,
    configuration::{self as common, ConfigurationTarget},
    error::{AppError, ErrorKind},
    export,
    inventory::DetailInput,
};
use komodo_client::api::read::{
    FindUser, GetPermission, GetUserGroup, ListPermissions, ListUserGroups,
    ListUserTargetPermissions, ListUsers, ServiceUserQueryBehavior,
};
use komodo_client::entities::permission::UserTarget;
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
mod change;
pub(crate) use change::guard_write;
pub use change::{apply, preview};
pub(crate) const OP: &str = "access_administration";
const MAX_SCAN: usize = 512;
pub(crate) const MAX_PRIVATE: usize = 256 * 1024;
pub const EFFECTS: &[&str] = &[
    "Every access apply requires both local grants and an enabled Core administrator; admin-status changes also require Core super-admin. Core remains authoritative. Self-disable, self-delete, self-admin revocation and protected super-admin targets are refused. There is no atomic protection against concurrent administrators.",
    "One scoped typed write at most, with complete private baselines, a final shared-transport recheck and independent read-back. Unknown replies stay uncertain even when current state matches; no automatic retry, credential fallback, execution update ID or rollback is provided. Core has no compare-and-swap.",
    "Direct grants are not effective access. Group, everyone, resource-base and type-wide inheritance can retain access after a direct grant or membership is removed. Only Core GetPermission for the current credential is authoritative; other-user direct records are never labelled effective.",
    "Core DeleteUser removes the user then attempts group cleanup, but leaves scoped permission and API-key records. DeleteUserGroup can remove the group before failing permission cleanup. Visible leftovers and observation gaps are reported separately; credential erasure or immediate cached-access revocation is not established. No hidden cleanup writes are submitted.",
    "Human/password provisioning, API-key issuance/rotation and authentication-provider configuration are excluded. User/Group write acknowledgements and authentication payloads are never returned raw. Core may retain administration data in its database/logs; these tools do not erase that exposure.",
];
pub(crate) fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, OP, None)
}
pub(crate) fn args(s: &'static str) -> AppError {
    AppError::arguments(OP, s)
}
pub(crate) fn stale() -> AppError {
    AppError::new(ErrorKind::StalePreview, OP, None)
}
pub(crate) fn denied() -> AppError {
    AppError {
        correction: Some(
            "Use the same enabled Core administrator credential; admin-status edits require super-admin. Core self/super-admin protections remain in force. No credential fallback is used.",
        ),
        ..AppError::new(ErrorKind::PermissionDenied, OP, None)
    }
}
pub(crate) fn id(v: &Value) -> Result<&str, AppError> {
    v["_id"]["$oid"]
        .as_str()
        .filter(|s| crate::deletion::object_id(s))
        .ok_or_else(invalid)
}
pub(crate) fn label(client: &CoreClient, s: &str) -> String {
    if s.contains("://") || crate::logs::redact_log(client, s) != s {
        client.redaction_marker().into()
    } else {
        common::bounded_text(client, s, 128)
    }
}
pub(crate) fn name(s: &str) -> bool {
    !s.trim().is_empty() && s.len() <= 128 && !s.chars().any(char::is_control)
}
pub(crate) fn private_bound(v: &Value) -> Result<(), AppError> {
    if v.to_string().len() > MAX_PRIVATE {
        Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None))
    } else {
        Ok(())
    }
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Level {
    None,
    Read,
    Execute,
    Write,
}
impl Level {
    pub(crate) fn core(self) -> &'static str {
        match self {
            Self::None => "None",
            Self::Read => "Read",
            Self::Execute => "Execute",
            Self::Write => "Write",
        }
    }
    fn parse(v: &Value) -> Result<Self, AppError> {
        match v.as_str() {
            Some("None") => Ok(Self::None),
            Some("Read") => Ok(Self::Read),
            Some("Execute") => Ok(Self::Execute),
            Some("Write") => Ok(Self::Write),
            _ => Err(invalid()),
        }
    }
}
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Specific {
    Terminal,
    Attach,
    Inspect,
    Logs,
    Processes,
}
impl Specific {
    pub(crate) fn core(self) -> &'static str {
        match self {
            Self::Terminal => "Terminal",
            Self::Attach => "Attach",
            Self::Inspect => "Inspect",
            Self::Logs => "Logs",
            Self::Processes => "Processes",
        }
    }
    fn parse(v: &Value) -> Option<Self> {
        match v.as_str() {
            Some("Terminal") => Some(Self::Terminal),
            Some("Attach") => Some(Self::Attach),
            Some("Inspect") => Some(Self::Inspect),
            Some("Logs") => Some(Self::Logs),
            Some("Processes") => Some(Self::Processes),
            _ => None,
        }
    }
}
// DetailInput rejects explicit null selectors on input. Preserve the same wire
// shape in the private transport proof so it can be decoded and revalidated.
fn serialize_selector<S: serde::Serializer>(
    value: &DetailInput,
    serializer: S,
) -> Result<S::Ok, S::Error> {
    use serde::ser::SerializeMap;
    let mut map = serializer.serialize_map(None)?;
    if let Some(id) = &value.id {
        map.serialize_entry("id", id)?;
    }
    if let Some(name) = &value.name {
        map.serialize_entry("name", name)?;
    }
    map.end()
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Subject {
    User {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
    },
    Group {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
    },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PermissionScope {
    Resource {
        resource_kind: export::Kind,
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
    },
    /// Explicitly covers all current/future resources of this type.
    ResourceType { resource_kind: export::Kind },
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct GrantPatch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "common::present"
    )]
    #[schemars(with = "Level")]
    pub level: Option<Level>,
    #[serde(default)]
    pub add_specific: Vec<Specific>,
    #[serde(default)]
    pub remove_specific: Vec<Specific>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum Change {
    CreateServiceUser {
        username: String,
        #[serde(default)]
        description: String,
    },
    SetServiceUserDescription {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
        description: String,
    },
    SetUserBase {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "common::present"
        )]
        #[schemars(with = "bool")]
        enabled: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "common::present"
        )]
        #[schemars(with = "bool")]
        create_servers: Option<bool>,
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "common::present"
        )]
        #[schemars(with = "bool")]
        create_builds: Option<bool>,
    },
    SetUserAdmin {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
        admin: bool,
    },
    DeleteUser {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
    },
    CreateGroup {
        name: String,
    },
    RenameGroup {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
        name: String,
    },
    DeleteGroup {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
    },
    SetGroupMember {
        #[serde(serialize_with = "serialize_selector")]
        group: DetailInput,
        #[serde(serialize_with = "serialize_selector")]
        user: DetailInput,
        present: bool,
    },
    SetGroupEveryone {
        #[serde(serialize_with = "serialize_selector")]
        target: DetailInput,
        everyone: bool,
    },
    ChangePermission {
        subject: Subject,
        scope: PermissionScope,
        patch: GrantPatch,
    },
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub change: Change,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub change: Change,
    pub preview_id: String,
}
#[derive(Clone, Copy, Default, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum UserKind {
    #[default]
    All,
    Human,
    Service,
}
fn first() -> usize {
    1
}
fn limit() -> usize {
    20
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    #[serde(default = "first")]
    pub page: usize,
    #[serde(default = "limit")]
    pub limit: usize,
}
impl ListInput {
    fn offset(&self) -> Result<usize, AppError> {
        if !(1..=1_000_000).contains(&self.page) || !(1..=50).contains(&self.limit) {
            return Err(args("Use page 1–1000000 and limit 1–50"));
        }
        Ok((self.page - 1) * self.limit)
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UserListInput {
    #[serde(default = "first")]
    pub page: usize,
    #[serde(default = "limit")]
    pub limit: usize,
    #[serde(default)]
    pub kind: UserKind,
}
#[derive(Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum DirectSubject {
    CurrentCaller {},
    Selected { subject: Subject },
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DirectInput {
    pub subject: DirectSubject,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct EffectiveInput {
    pub resource_kind: export::Kind,
    pub target: DetailInput,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct GrantView {
    pub level: Level,
    pub specific: Vec<Specific>,
    pub unknown_specific_count: usize,
    pub complete: bool,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct TypeGrant {
    pub resource_kind: export::Kind,
    pub grant: GrantView,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct UserView {
    pub id: Option<String>,
    pub username: String,
    pub service_user: bool,
    pub enabled: bool,
    pub admin: bool,
    pub super_admin: bool,
    pub create_servers: bool,
    pub create_builds: bool,
    pub direct_type_grants: Vec<TypeGrant>,
    pub grants_complete: bool,
    pub authentication_details: &'static str,
    pub service_description: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct GroupView {
    pub id: Option<String>,
    pub name: String,
    pub everyone: bool,
    pub member_ids: Vec<String>,
    pub member_count: usize,
    pub members_complete: bool,
    pub direct_type_grants: Vec<TypeGrant>,
    pub grants_complete: bool,
    pub visibility_limit: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: usize,
    pub next_page: Option<usize>,
    pub scanned: usize,
    pub scan_truncated: bool,
    pub visibility_limit: &'static str,
    pub mutation_requests: u8,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct SubjectView {
    pub kind: &'static str,
    pub id: String,
    pub name: String,
}
#[derive(Clone, Serialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScopeView {
    Resource {
        resource_kind: export::Kind,
        id: String,
        name: Option<String>,
    },
    ResourceType {
        resource_kind: export::Kind,
    },
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct PermissionView {
    pub subject: SubjectView,
    pub resource_kind: crate::history::ResourceKind,
    pub resource_id: String,
    pub grant: GrantView,
}
#[derive(Serialize, JsonSchema)]
pub struct DirectPermissions {
    pub subject: SubjectView,
    pub records: Vec<PermissionView>,
    pub direct_type_grants: Vec<TypeGrant>,
    pub complete: bool,
    pub evidence: &'static str,
    pub mutation_requests: u8,
}
#[derive(Serialize, JsonSchema)]
pub struct EffectivePermission {
    pub caller_id: String,
    pub target: ScopeView,
    pub grant: GrantView,
    pub target_identity_verified: bool,
    pub evidence: &'static str,
    pub mutation_requests: u8,
}
#[derive(Clone, Serialize, JsonSchema)]
#[serde(tag = "kind", content = "details", rename_all = "snake_case")]
pub enum EntityView {
    User(UserView),
    Group(GroupView),
    Permission {
        subject: SubjectView,
        scope: ScopeView,
        grant: GrantView,
        direct_record_exists: bool,
    },
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct CleanupReport {
    pub visible_group_references: Vec<SubjectView>,
    pub group_reference_count: Option<usize>,
    pub direct_permission_records_remaining: Option<usize>,
    pub observations_complete: bool,
    pub credential_cleanup: &'static str,
    pub errors: Vec<AppError>,
    pub evidence_limit: &'static str,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct AccessResult {
    pub action: &'static str,
    pub outcome: &'static str,
    pub resource: ConfigurationTarget,
    pub before: Option<EntityView>,
    pub after: Option<EntityView>,
    pub broad_scope: bool,
    pub requires_super_admin: bool,
    pub preview_id: Option<String>,
    pub writes_submitted: u8,
    pub acknowledgement_valid: bool,
    pub saved_state_matches: bool,
    pub submission_error: Option<AppError>,
    pub observation_error: Option<AppError>,
    pub cleanup: Option<CleanupReport>,
    pub effects: Vec<&'static str>,
}
const GROUP_VISIBILITY: &str = "Core non-admin group lists/details include explicit memberships only; implicit everyone groups can be absent. Visible direct records do not establish effective access. Core GetPermission is authoritative for the current credential.";
pub(crate) fn core_kind(k: export::Kind) -> &'static str {
    match k {
        export::Kind::Server => "Server",
        export::Kind::Swarm => "Swarm",
        export::Kind::Stack => "Stack",
        export::Kind::Deployment => "Deployment",
        export::Kind::Build => "Build",
        export::Kind::Repo => "Repo",
        export::Kind::Builder => "Builder",
        export::Kind::Alerter => "Alerter",
        export::Kind::Procedure => "Procedure",
        export::Kind::Action => "Action",
        export::Kind::ResourceSync => "ResourceSync",
    }
}
fn parse_kind(s: &str) -> Option<export::Kind> {
    [
        export::Kind::Server,
        export::Kind::Swarm,
        export::Kind::Stack,
        export::Kind::Deployment,
        export::Kind::Build,
        export::Kind::Repo,
        export::Kind::Builder,
        export::Kind::Alerter,
        export::Kind::Procedure,
        export::Kind::Action,
        export::Kind::ResourceSync,
    ]
    .into_iter()
    .find(|k| core_kind(*k) == s)
}
pub(crate) fn grant(v: &Value, strict: bool) -> Result<GrantView, AppError> {
    let (level, specific) = if v.is_string() {
        (Level::parse(v)?, Vec::new())
    } else {
        let m = v.as_object().ok_or_else(invalid)?;
        if strict
            && m.keys()
                .any(|k| !matches!(k.as_str(), "level" | "specific"))
        {
            return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
        }
        let level = Level::parse(&v["level"])?;
        let a = v["specific"].as_array().ok_or_else(invalid)?;
        if a.len() > 64 {
            return Err(invalid());
        }
        (level, a.clone())
    };
    let mut flags = BTreeSet::new();
    let mut unknown = 0;
    for s in &specific {
        if let Some(s) = Specific::parse(s) {
            flags.insert(s);
        } else {
            unknown += 1;
        }
    }
    if strict && unknown > 0 {
        return Err(AppError {
            correction: Some(
                "This changed grant contains unsupported specifics; preserve it rather than dropping unknown flags.",
            ),
            ..AppError::new(ErrorKind::UnsupportedTarget, OP, None)
        });
    }
    Ok(GrantView {
        level,
        specific: flags.into_iter().collect(),
        unknown_specific_count: unknown,
        complete: unknown == 0,
    })
}
pub(crate) fn wire_grant(g: &GrantView) -> Value {
    if g.specific.is_empty() {
        json!(g.level.core())
    } else {
        json!({"level":g.level.core(),"specific":g.specific.iter().map(|s|s.core()).collect::<Vec<_>>()})
    }
}
fn type_grants(v: &Value) -> Result<(Vec<TypeGrant>, bool), AppError> {
    let m = v.as_object().ok_or_else(invalid)?;
    if m.len() > 32 {
        return Err(invalid());
    }
    let mut complete = true;
    let mut out = Vec::new();
    for (k, v) in m {
        if let Some(k) = parse_kind(k) {
            let grant = grant(v, false)?;
            complete &= grant.complete;
            out.push(TypeGrant {
                resource_kind: k,
                grant,
            });
        } else {
            complete = false;
        }
    }
    Ok((out, complete))
}
pub(crate) fn raw_user(v: &Value) -> Result<(), AppError> {
    id(v)?;
    if !v["username"]
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.len() <= 500 && !s.chars().any(char::is_control))
        || [
            "enabled",
            "admin",
            "super_admin",
            "create_server_permissions",
            "create_build_permissions",
        ]
        .iter()
        .any(|k| !v[*k].is_boolean())
        || !v["all"].is_object()
        || !v["config"].is_object()
        || !v["config"]["type"]
            .as_str()
            .is_some_and(|s| matches!(s, "Local" | "Google" | "Github" | "Oidc" | "Service"))
    {
        return Err(invalid());
    }
    private_bound(v)
}
pub(crate) fn raw_group(v: &Value) -> Result<(), AppError> {
    id(v)?;
    if !v["name"]
        .as_str()
        .is_some_and(|s| !s.is_empty() && s.len() <= 500)
        || !v["everyone"].is_boolean()
        || !v["all"].is_object()
        || !v["users"].is_array()
    {
        return Err(invalid());
    }
    private_bound(v)
}
pub(crate) fn user_view(client: &CoreClient, v: &Value) -> Result<UserView, AppError> {
    raw_user(v)?;
    let (direct_type_grants, grants_complete) = type_grants(&v["all"])?;
    Ok(UserView {
        id: Some(id(v)?.into()),
        username: label(client, v["username"].as_str().expect("checked")),
        service_user: v["config"]["type"] == "Service",
        enabled: v["enabled"] == true,
        admin: v["admin"] == true,
        super_admin: v["super_admin"] == true,
        create_servers: v["create_server_permissions"] == true,
        create_builds: v["create_build_permissions"] == true,
        direct_type_grants,
        grants_complete,
        authentication_details: "omitted",
        service_description: "omitted; use original private input for description edits",
    })
}
pub(crate) fn group_view(client: &CoreClient, v: &Value) -> Result<GroupView, AppError> {
    raw_group(v)?;
    let users = v["users"].as_array().expect("checked");
    let member_ids = users
        .iter()
        .filter_map(Value::as_str)
        .filter(|s| crate::deletion::object_id(s))
        .take(32)
        .map(String::from)
        .collect::<Vec<_>>();
    let (direct_type_grants, grants_complete) = type_grants(&v["all"])?;
    Ok(GroupView {
        id: Some(id(v)?.into()),
        name: label(client, v["name"].as_str().expect("checked")),
        everyone: v["everyone"] == true,
        members_complete: member_ids.len() == users.len(),
        member_ids,
        member_count: users.len(),
        direct_type_grants,
        grants_complete,
        visibility_limit: GROUP_VISIBILITY,
    })
}
pub(crate) async fn actor(client: &CoreClient) -> Result<Value, AppError> {
    let u = client.current_caller().await?;
    if u["enabled"] != true || u["admin"] != true {
        return Err(denied());
    }
    Ok(u)
}
fn rows(raw: Value, complete: bool) -> Result<(Vec<Value>, bool), AppError> {
    let a = raw.as_array().ok_or_else(invalid)?;
    let truncated = a.len() > MAX_SCAN;
    if complete && truncated {
        return Err(AppError::new(ErrorKind::IncompleteResolution, OP, None));
    }
    Ok((a.iter().take(MAX_SCAN).cloned().collect(), truncated))
}
pub(crate) async fn users(
    client: &CoreClient,
    complete: bool,
    kind: UserKind,
) -> Result<(Vec<Value>, bool), AppError> {
    let raw = client
        .read_references(ListUsers {
            service_users: match kind {
                UserKind::All => ServiceUserQueryBehavior::Include,
                UserKind::Human => ServiceUserQueryBehavior::Exclude,
                UserKind::Service => ServiceUserQueryBehavior::Only,
            },
        })
        .await?;
    let (mut a, t) = rows(raw, complete)?;
    let mut ids = BTreeSet::new();
    for v in &a {
        raw_user(v)?;
        if !ids.insert(id(v)?.to_owned()) {
            return Err(invalid());
        }
    }
    a.sort_by(|a, b| a["username"].as_str().cmp(&b["username"].as_str()));
    Ok((a, t))
}
pub(crate) async fn groups(
    client: &CoreClient,
    complete: bool,
) -> Result<(Vec<Value>, bool), AppError> {
    let (mut a, t) = rows(client.read_references(ListUserGroups {}).await?, complete)?;
    let mut ids = BTreeSet::new();
    for v in &a {
        raw_group(v)?;
        if !ids.insert(id(v)?.to_owned()) {
            return Err(invalid());
        }
    }
    a.sort_by(|a, b| a["name"].as_str().cmp(&b["name"].as_str()));
    Ok((a, t))
}
fn select(a: &[Value], target: &DetailInput, key: &str) -> Result<Value, AppError> {
    export::validate_selector(target)?;
    let mut matches = a.iter().filter(|v| {
        if let Some(i) = &target.id {
            v["_id"]["$oid"]
                .as_str()
                .is_some_and(|s| s.eq_ignore_ascii_case(i))
        } else {
            v[key].as_str() == target.name.as_deref()
        }
    });
    let first = matches
        .next()
        .ok_or_else(|| AppError::new(ErrorKind::NotFound, OP, None))?;
    if matches.next().is_some() {
        return Err(AppError::new(ErrorKind::AmbiguousTarget, OP, None));
    }
    Ok(first.clone())
}
pub(crate) async fn user(client: &CoreClient, target: &DetailInput) -> Result<Value, AppError> {
    let (a, _) = users(client, true, UserKind::All).await?;
    let listed = select(&a, target, "username")?;
    let full = client
        .read_references(FindUser {
            user: id(&listed)?.into(),
        })
        .await?;
    raw_user(&full)?;
    if id(&full)? != id(&listed)? || full["username"] != listed["username"] {
        return Err(invalid());
    }
    Ok(full)
}
pub(crate) async fn group(client: &CoreClient, target: &DetailInput) -> Result<Value, AppError> {
    let (a, _) = groups(client, true).await?;
    let listed = select(&a, target, "name")?;
    let full = client
        .read_references(GetUserGroup {
            user_group: id(&listed)?.into(),
        })
        .await?;
    raw_group(&full)?;
    if id(&full)? != id(&listed)? || full["name"] != listed["name"] {
        return Err(invalid());
    }
    Ok(full)
}
pub(crate) async fn subject(
    client: &CoreClient,
    s: &Subject,
) -> Result<(SubjectView, Value), AppError> {
    let (kind, v, key) = match s {
        Subject::User { target } => ("user", user(client, target).await?, "username"),
        Subject::Group { target } => ("group", group(client, target).await?, "name"),
    };
    Ok((
        SubjectView {
            kind,
            id: id(&v)?.into(),
            name: label(client, v[key].as_str().expect("checked")),
        },
        v,
    ))
}
pub(crate) fn wire_subject(s: &SubjectView) -> UserTarget {
    if s.kind == "user" {
        UserTarget::User(s.id.clone())
    } else {
        UserTarget::UserGroup(s.id.clone())
    }
}
pub(crate) async fn permissions(
    client: &CoreClient,
    s: &SubjectView,
    complete: bool,
) -> Result<(Vec<Value>, bool), AppError> {
    let raw = client
        .read_references(ListUserTargetPermissions {
            user_target: wire_subject(s),
        })
        .await?;
    permission_rows(raw, s, complete)
}
fn permission_rows(
    raw: Value,
    s: &SubjectView,
    complete: bool,
) -> Result<(Vec<Value>, bool), AppError> {
    let (mut a, t) = rows(raw, complete)?;
    let expected = serde_json::to_value(wire_subject(s)).expect("subject");
    let mut targets = BTreeSet::new();
    for v in &a {
        if v["user_target"] != expected
            || !v["resource_target"].is_object()
            || !v["specific"].is_array()
        {
            return Err(invalid());
        }
        let _: komodo_client::entities::ResourceTarget =
            serde_json::from_value(v["resource_target"].clone()).map_err(|_| invalid())?;
        Level::parse(&v["level"])?;
        let key = v["resource_target"].to_string();
        if !targets.insert(key) {
            return Err(invalid());
        }
    }
    a.sort_by_key(|v| v["resource_target"].to_string());
    Ok((a, t))
}
fn record_view(s: &SubjectView, v: &Value) -> Result<PermissionView, AppError> {
    let target: komodo_client::entities::ResourceTarget =
        serde_json::from_value(v["resource_target"].clone()).map_err(|_| invalid())?;
    let (k, id) = target.extract_variant_id();
    if !crate::deletion::object_id(id) {
        return Err(invalid());
    }
    Ok(PermissionView {
        subject: s.clone(),
        resource_kind: k.into(),
        resource_id: id.clone(),
        grant: grant(&json!({"level":v["level"],"specific":v["specific"]}), false)?,
    })
}
pub(crate) async fn resource(
    client: &CoreClient,
    kind: export::Kind,
    target: &DetailInput,
) -> Result<(ScopeView, Value), AppError> {
    let selected = export::select(
        client,
        export::Target {
            kind,
            target: target.clone(),
        },
    )
    .await?;
    macro_rules! raw {
        ($ty:ident,$field:ident) => {
            client
                .read_references(komodo_client::api::read::$ty {
                    $field: selected.id.clone(),
                })
                .await?
        };
    }
    let raw = match kind {
        export::Kind::Server => raw!(GetServer, server),
        export::Kind::Swarm => raw!(GetSwarm, swarm),
        export::Kind::Stack => raw!(GetStack, stack),
        export::Kind::Deployment => raw!(GetDeployment, deployment),
        export::Kind::Build => raw!(GetBuild, build),
        export::Kind::Repo => raw!(GetRepo, repo),
        export::Kind::Builder => raw!(GetBuilder, builder),
        export::Kind::Alerter => raw!(GetAlerter, alerter),
        export::Kind::Procedure => raw!(GetProcedure, procedure),
        export::Kind::Action => raw!(GetAction, action),
        export::Kind::ResourceSync => raw!(GetResourceSync, sync),
    };
    if id(&raw)? != selected.id || raw["name"] != selected.name || !raw["config"].is_object() {
        return Err(invalid());
    }
    private_bound(&raw)?;
    Ok((
        ScopeView::Resource {
            resource_kind: kind,
            id: selected.id,
            name: Some(label(client, &selected.name)),
        },
        raw,
    ))
}
fn page<T>(
    mut items: Vec<T>,
    input: ListInput,
    truncated: bool,
    visibility: &'static str,
) -> Result<Page<T>, AppError> {
    let offset = input.offset()?;
    let scanned = items.len();
    let more = offset.saturating_add(input.limit) < scanned;
    let take = items.drain(..).skip(offset).take(input.limit).collect();
    Ok(Page {
        items: take,
        page: input.page,
        next_page: more.then_some(input.page + 1),
        scanned,
        scan_truncated: truncated,
        visibility_limit: visibility,
        mutation_requests: 0,
    })
}
pub async fn list_users(
    client: &CoreClient,
    input: UserListInput,
) -> Result<Page<UserView>, AppError> {
    let paging = ListInput {
        page: input.page,
        limit: input.limit,
    };
    paging.offset()?;
    actor(client).await?;
    let (a, t) = users(&client.bounded_evidence(), false, input.kind).await?;
    page(
        a.iter()
            .map(|v| user_view(client, v))
            .collect::<Result<_, _>>()?,
        paging,
        t,
        "Core admin-only user directory; local pagination/512-entry scan/2 MiB body do not bound Core's internal enumeration. Authentication and descriptions omitted.",
    )
}
pub async fn get_user(client: &CoreClient, input: DetailInput) -> Result<UserView, AppError> {
    export::validate_selector(&input)?;
    actor(client).await?;
    user_view(client, &user(&client.bounded_evidence(), &input).await?)
}
pub async fn list_groups(
    client: &CoreClient,
    input: ListInput,
) -> Result<Page<GroupView>, AppError> {
    input.offset()?;
    let (a, t) = groups(&client.bounded_evidence(), false).await?;
    page(
        a.iter()
            .map(|v| group_view(client, v))
            .collect::<Result<_, _>>()?,
        input,
        t,
        GROUP_VISIBILITY,
    )
}
pub async fn get_group(client: &CoreClient, input: DetailInput) -> Result<GroupView, AppError> {
    group_view(client, &group(&client.bounded_evidence(), &input).await?)
}
pub async fn direct(
    client: &CoreClient,
    input: DirectInput,
) -> Result<DirectPermissions, AppError> {
    let b = client.bounded_evidence();
    let (s, all, a, t, evidence) = match input.subject {
        DirectSubject::CurrentCaller {} => {
            let u = client.current_caller().await?;
            let s = SubjectView {
                kind: "user",
                id: u["id"].as_str().expect("validated").into(),
                name: label(client, u["username"].as_str().expect("validated")),
            };
            let (a, t) = permission_rows(b.read_references(ListPermissions {}).await?, &s, false)?;
            (
                s,
                Vec::new(),
                a,
                t,
                "Direct current-user scoped records only. User type-wide grants and all group/resource inheritance are excluded; use get_effective_permission for actual current-caller access.",
            )
        }
        DirectSubject::Selected { subject: selection } => {
            actor(client).await?;
            let (s, v) = subject(&b, &selection).await?;
            let (all, all_complete) = type_grants(&v["all"])?;
            let (a, t) = permissions(&b, &s, false).await?;
            (
                s,
                all,
                a,
                t || !all_complete,
                "Direct selected-user/group records and type-wide grants only, read with Core administrator authority. Not an effective-permission result for that user or any group member.",
            )
        }
    };
    let records = a
        .iter()
        .map(|v| record_view(&s, v))
        .collect::<Result<Vec<_>, _>>()?;
    let complete = !t && records.iter().all(|r| r.grant.complete);
    Ok(DirectPermissions {
        subject: s,
        records,
        direct_type_grants: all,
        complete,
        evidence,
        mutation_requests: 0,
    })
}
pub async fn effective(
    client: &CoreClient,
    input: EffectiveInput,
) -> Result<EffectivePermission, AppError> {
    export::validate_selector(&input.target)?;
    let b = client.bounded_evidence();
    let caller = client.current_caller().await?;
    let (mut target, mut verified) = if let Some(id) = &input.target.id {
        (
            ScopeView::Resource {
                resource_kind: input.resource_kind,
                id: id.to_ascii_lowercase(),
                name: None,
            },
            false,
        )
    } else {
        let (target, _) = resource(&b, input.resource_kind, &input.target).await?;
        (target, true)
    };
    let ScopeView::Resource { id, .. } = &target else {
        unreachable!()
    };
    let target_wire =
        serde_json::from_value(json!({"type": core_kind(input.resource_kind), "id": id}))
            .expect("typed target");
    let g = grant(
        &b.read_references(GetPermission {
            target: target_wire,
        })
        .await?,
        true,
    )?;
    // A stable-ID no-access query needs no privileged inventory fallback. Core's
    // admin shortcut always reports Write; any readable result is independently
    // resolved so that a phantom ID cannot manufacture an existing resource.
    if !verified && g.level != Level::None {
        let (resolved, _) = resource(&b, input.resource_kind, &input.target).await?;
        target = resolved;
        verified = true;
    }
    Ok(EffectivePermission {
        caller_id: caller["id"].as_str().expect("validated").into(),
        target,
        grant: g,
        target_identity_verified: verified,
        evidence: "Core GetPermission for this current credential, including resource-base, direct, type-wide, group/everyone inheritance and specifics. Names and readable stable IDs are positively resolved; a no-access stable-ID result discloses no resource metadata and does not independently establish identity/existence. No admin-shortcut existence claim, other-user calculation or future authorization guarantee; authority/definition can race.",
        mutation_requests: 0,
    })
}
