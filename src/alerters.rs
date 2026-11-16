//! Private Alerter configuration and one explicit saved-target delivery test.
use std::time::Instant;

use crate::{
    client::CoreClient,
    configuration::{
        self as common, ConfigurationOutcome, ConfigurationResult, ConfigurationTarget,
        FieldChange, Preview, present,
    },
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput, ListInput, Page},
    operations::{
        self, ConfiguredEffects, OperationResource, RequestedScope, SubmissionReceipt,
        WorkloadKind, WorkloadOperation,
    },
};
use komodo_client::{
    api::{
        execute::TestAlerter,
        read::GetAlerter,
        write::{CreateAlerter, UpdateAlerter},
    },
    entities::{
        Operation, ResourceTarget,
        alerter::{Alerter, AlerterConfig, AlerterListItem, PartialAlerterConfig},
        update::{Update, UpdateStatus},
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const OP: &str = "alerter_configuration";
pub const DELIVERY_LIMIT: &str = "TestAlerter addresses only this saved Alerter, never enables it and never broadcasts SendAlert. Disabled Alerters fail without delivery. Tests bypass type/resource filters but honor maintenance; Core success can therefore mean suppression without a provider attempt. Core/provider reports do not prove receipt. Independently inspect an owned receiver. No automatic resend follows uncertainty.";
const EFFECT: &str = "Saving never submits TestAlerter or SendAlert, but enabled routing, filters, maintenance and interpolated endpoint changes can affect future automatic delivery. Every existing-definition apply requires both local grants regardless of the observed enabled state; creation defaults disabled. Core requires admin to create, resource Write to update and Execute to test. References remain literal and Core alone interpolates URLs at send time; Ntfy email is literal.";

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct UrlPatch {
    /// Original URL or Core reference expression. Omit to preserve the same variant.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub url: Option<String>,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EmailChange {
    Clear {},
    Set { value: String },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum EndpointPatch {
    Custom(UrlPatch),
    Slack(UrlPatch),
    Discord(UrlPatch),
    Pushover(UrlPatch),
    Ntfy {
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "String")]
        url: Option<String>,
        /// Omit to preserve; clear explicitly removes the literal X-Email recipient.
        #[serde(
            default,
            skip_serializing_if = "Option::is_none",
            deserialize_with = "present"
        )]
        #[schemars(with = "EmailChange")]
        email: Option<EmailChange>,
    },
}
impl EndpointPatch {
    fn fields(&self) -> (&'static str, Option<&str>, Option<&EmailChange>) {
        match self {
            Self::Custom(p) => ("Custom", p.url.as_deref(), None),
            Self::Slack(p) => ("Slack", p.url.as_deref(), None),
            Self::Discord(p) => ("Discord", p.url.as_deref(), None),
            Self::Pushover(p) => ("Pushover", p.url.as_deref(), None),
            Self::Ntfy { url, email } => ("Ntfy", url.as_deref(), email.as_ref()),
        }
    }
    fn prepare(&self, before: Option<&Value>) -> Result<Value, AppError> {
        let (kind, url, email) = self.fields();
        let same = before.is_some_and(|b| b["type"] == kind);
        let mut value = if same {
            before.expect("same").clone()
        } else {
            json!({"type":kind,"params":{}})
        };
        // SDK structs cannot round-trip extensions in a replaced endpoint.
        if same
            && (value
                .as_object()
                .is_none_or(|m| m.keys().any(|k| !matches!(k.as_str(), "type" | "params")))
                || value["params"].as_object().is_none_or(|m| {
                    m.keys()
                        .any(|k| k != "url" && !(kind == "Ntfy" && k == "email"))
                }))
        {
            return Err(args(
                "This endpoint contains unsupported fields. Preserve it by omitting endpoint; do not replace a partial projection.",
            ));
        }
        if let Some(url) = url {
            if url.is_empty() || url.len() > 8192 || url.chars().any(char::is_control) {
                return Err(args(
                    "Supply an original nonempty URL/reference expression of at most 8192 bytes without controls",
                ));
            }
            value["params"]["url"] = json!(url);
        } else if !same {
            return Err(args(
                "Creation or an endpoint variant change requires its original URL or Core reference expression",
            ));
        }
        if kind == "Ntfy" {
            if !same {
                value["params"]["email"] = Value::Null;
            }
            if let Some(change) = email {
                value["params"]["email"] = match change {
                    EmailChange::Clear {} => Value::Null,
                    EmailChange::Set { value } => {
                        if value.is_empty()
                            || value.len() > 320
                            || value.chars().any(char::is_control)
                        {
                            return Err(args(
                                "Supply a literal Ntfy email recipient of at most 320 bytes, or clear explicitly",
                            ));
                        }
                        json!(value)
                    }
                };
            }
        }
        Ok(value)
    }
}

macro_rules! alert_types {($($v:ident),* $(,)?)=>{
    #[derive(Clone, Serialize, Deserialize, JsonSchema)]
    pub enum AlertType { $($v),* }
};}
alert_types!(
    None,
    Test,
    SwarmUnhealthy,
    ServerUnreachable,
    ServerCpu,
    ServerMem,
    ServerDisk,
    ServerVersionMismatch,
    ContainerStateChange,
    DeploymentImageUpdateAvailable,
    DeploymentAutoUpdated,
    StackStateChange,
    StackImageUpdateAvailable,
    StackAutoUpdated,
    AwsBuilderTerminationFailed,
    ResourceSyncPendingUpdates,
    BuildFailed,
    RepoBuildFailed,
    ProcedureFailed,
    ActionFailed,
    ScheduleRun,
    Custom
);

#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum FilterTarget {
    System {},
    Resource {
        resource_kind: crate::export::Kind,
        target: DetailInput,
    },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
pub enum Weekday {
    Monday,
    Tuesday,
    Wednesday,
    Thursday,
    Friday,
    Saturday,
    Sunday,
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum MaintenanceSchedule {
    Daily {},
    Weekly { day_of_week: Weekday },
    OneTime { date: String },
}
#[derive(Clone, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct MaintenanceWindow {
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub schedule: MaintenanceSchedule,
    pub hour: u8,
    pub minute: u8,
    /// Pinned Core wraps time-of-day arithmetic. Do not use 1440 as all-day maintenance.
    pub duration_minutes: u32,
    #[serde(default)]
    pub timezone: String,
    pub enabled: bool,
}
impl MaintenanceWindow {
    fn value(&self) -> Result<Value, AppError> {
        if self.name.trim().is_empty()
            || self.name.len() > 128
            || self.description.len() > 1024
            || self.hour > 23
            || self.minute > 59
            || !(1..1440).contains(&self.duration_minutes)
            || self.timezone.len() > 128
            || [&self.name, &self.description, &self.timezone]
                .iter()
                .any(|s| s.chars().any(char::is_control))
        {
            return Err(args(
                "Use bounded maintenance text, hour 0–23, minute 0–59 and duration 1–1439 minutes; Core evaluates timezones and activity authoritatively",
            ));
        }
        let (kind, day, date) = match &self.schedule {
            MaintenanceSchedule::Daily {} => ("Daily", json!(""), ""),
            MaintenanceSchedule::Weekly { day_of_week } => ("Weekly", json!(day_of_week), ""),
            MaintenanceSchedule::OneTime { date } => {
                if !valid_date(date) {
                    return Err(args("One-time maintenance needs a valid YYYY-MM-DD date"));
                }
                ("OneTime", json!(""), date.as_str())
            }
        };
        Ok(
            json!({"name":self.name,"description":self.description,"schedule_type":kind,"day_of_week":day,"date":date,"hour":self.hour,"minute":self.minute,"duration_minutes":self.duration_minutes,"timezone":self.timezone,"enabled":self.enabled}),
        )
    }
}
fn valid_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10
        || b[4] != b'-'
        || b[7] != b'-'
        || b.iter()
            .enumerate()
            .any(|(i, c)| i != 4 && i != 7 && !c.is_ascii_digit())
    {
        return false;
    }
    let y = s[..4].parse::<u32>().unwrap();
    let m = s[5..7].parse::<usize>().unwrap();
    let d = s[8..].parse::<u32>().unwrap();
    let days = [
        31,
        if y.is_multiple_of(400) || (y.is_multiple_of(4) && !y.is_multiple_of(100)) {
            29
        } else {
            28
        },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    y > 0 && (1..=12).contains(&m) && d > 0 && d <= days[m - 1]
}

#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "bool")]
    pub enabled: Option<bool>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "EndpointPatch")]
    pub endpoint: Option<EndpointPatch>,
    /// [] restores unrestricted alert types. Omit preserves.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<AlertType>")]
    pub alert_types: Option<Vec<AlertType>>,
    /// [] restores unrestricted inclusion. Exclusions still apply. Omit preserves.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<FilterTarget>")]
    pub resources: Option<Vec<FilterTarget>>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<FilterTarget>")]
    pub except_resources: Option<Vec<FilterTarget>>,
    /// [] clears all windows. Omit preserves unknown/private saved fields.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Vec<MaintenanceWindow>")]
    pub maintenance_windows: Option<Vec<MaintenanceWindow>>,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub target: DetailInput,
    pub patch: Patch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub target: DetailInput,
    pub patch: Patch,
    pub preview_id: String,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateInput {
    pub name: String,
    pub config: Patch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct CreateApplyInput {
    pub name: String,
    pub config: Patch,
    pub preview_id: String,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct TestInput {
    pub target: DetailInput,
    pub acknowledge_delivery: bool,
}

#[derive(Clone, Serialize, JsonSchema)]
pub struct Summary {
    pub id: String,
    pub name: String,
    pub template: bool,
    pub enabled: bool,
    pub endpoint_type: String,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionSummary {
    pub enabled: bool,
    pub endpoint_type: String,
    pub endpoint_details: &'static str,
    pub ntfy_email_configured: bool,
    pub alert_types: Vec<String>,
    pub all_alert_types: bool,
    pub resources: Vec<crate::history::HistoryTarget>,
    pub all_resources: bool,
    pub except_resources: Vec<crate::history::HistoryTarget>,
    /// Names/descriptions are omitted; clock fields are allowlisted metadata only.
    pub maintenance_windows: Vec<Value>,
    pub projection_complete: bool,
    pub maintenance_evaluation: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct Detail {
    pub summary: Summary,
    pub definition: DefinitionSummary,
}
#[derive(Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Option<DefinitionSummary>,
    pub after: DefinitionSummary,
    pub requires_operate_to_apply: bool,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TestObservation {
    pub core_report: &'static str,
    pub provider_attempt: &'static str,
    pub independently_observed_receipt: &'static str,
    pub evidence_limit: &'static str,
}
pub(crate) fn observe_test(u: &Update) -> Option<TestObservation> {
    (u.operation == Operation::TestAlerter).then_some(TestObservation {
        core_report: match (u.status, u.success) {
            (UpdateStatus::Complete, true) => "success_reported",
            (UpdateStatus::Complete, false) => "failure_reported",
            _ => "pending",
        },
        provider_attempt: "not_established",
        independently_observed_receipt: "not_verified",
        evidence_limit: DELIVERY_LIMIT,
    })
}
#[derive(Serialize, JsonSchema)]
pub struct TestResult {
    pub receipt: SubmissionReceipt,
    pub delivery_limit: &'static str,
}
fn args(s: &'static str) -> AppError {
    AppError::arguments(OP, s)
}
fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, OP, None)
}
fn endpoint_kind(v: &Value) -> Option<&str> {
    v["type"]
        .as_str()
        .filter(|s| matches!(*s, "Custom" | "Slack" | "Discord" | "Ntfy" | "Pushover"))
}
fn targets(v: &Value) -> Vec<crate::history::HistoryTarget> {
    v.as_array()
        .into_iter()
        .flatten()
        .take(16)
        .filter_map(|v| {
            let target: ResourceTarget = serde_json::from_value(v.clone()).ok()?;
            let (k, id) = target.extract_variant_id();
            ((k.as_ref() == "System" && id == "system") || crate::deletion::object_id(id)).then(
                || crate::history::HistoryTarget {
                    kind: k.into(),
                    id: id.clone(),
                },
            )
        })
        .collect()
}
fn maintenance_metadata(w: &Value) -> Value {
    let schedule = match w["schedule_type"].as_str() {
        Some(s @ ("Daily" | "Weekly" | "OneTime")) => Some(s),
        _ => None,
    };
    let mut safe = json!({
        "enabled": w["enabled"].as_bool(),
        "hour": w["hour"].as_u64().filter(|n| *n <= 23),
        "minute": w["minute"].as_u64().filter(|n| *n <= 59),
        "duration_minutes": w["duration_minutes"].as_u64(),
        "schedule_type": schedule,
    });
    if let Some(day) = w["day_of_week"]
        .as_str()
        .filter(|d| serde_json::from_value::<Weekday>(json!(d)).is_ok())
    {
        safe["day_of_week"] = json!(day);
    }
    if let Some(date) = w["date"].as_str().filter(|d| valid_date(d)) {
        safe["date"] = json!(date);
    }
    if let Some(tz) = w["timezone"].as_str().filter(|t| {
        t.len() <= 128
            && t.bytes()
                .all(|b| b.is_ascii_alphabetic() || b"_/-+:0123456789".contains(&b))
    }) {
        safe["timezone"] = json!(tz);
    }
    safe
}
fn summary(v: &Value) -> Result<DefinitionSummary, AppError> {
    let c: AlerterConfig = serde_json::from_value(v.clone()).map_err(|_| invalid())?;
    let resources = targets(&v["resources"]);
    let except = targets(&v["except_resources"]);
    let windows = v["maintenance_windows"].as_array().ok_or_else(invalid)?;
    let maintenance_windows = windows.iter().take(16).map(maintenance_metadata).collect();
    Ok(DefinitionSummary {
        enabled: c.enabled,
        endpoint_type: endpoint_kind(&v["endpoint"]).ok_or_else(invalid)?.into(),
        endpoint_details: "URL, credentials, references and literal email omitted; use original private input to change them",
        ntfy_email_configured: v["endpoint"]["type"] == "Ntfy"
            && v["endpoint"]["params"]["email"]
                .as_str()
                .is_some_and(|s| !s.is_empty()),
        alert_types: c
            .alert_types
            .iter()
            .take(32)
            .map(|x| {
                serde_json::to_value(x)
                    .expect("enum")
                    .as_str()
                    .expect("enum string")
                    .into()
            })
            .collect(),
        all_alert_types: c.alert_types.is_empty(),
        projection_complete: c.resources.len() == resources.len()
            && c.except_resources.len() == except.len()
            && windows.len() <= 16
            && c.alert_types.len() <= 32,
        resources,
        all_resources: c.resources.is_empty(),
        except_resources: except,
        maintenance_windows,
        maintenance_evaluation: "Core evaluates activity/timezones; invalid saved fields may be ignored/defaulted. Pinned time-of-day arithmetic wraps across midnight; no active/all-day suppression claim is made.",
    })
}
fn item(client: &CoreClient, r: AlerterListItem) -> Summary {
    Summary {
        id: r.id,
        name: common::bounded_text(client, &r.name, 128),
        template: r.template,
        enabled: r.info.enabled,
        endpoint_type: r.info.endpoint_type.to_string(),
    }
}
pub async fn list(client: &CoreClient, input: ListInput) -> Result<Page<Summary>, AppError> {
    let p = inventory::list_alerter_records(&client.bounded_evidence(), input).await?;
    Ok(Page {
        items: p.items.into_iter().map(|r| item(client, r)).collect(),
        page: p.page,
        page_size: p.page_size,
        has_more: p.has_more,
        next_page: p.next_page,
    })
}
// Full raw configuration remains private, including fields absent from SDK structs.
async fn snapshot(client: &CoreClient, target: DetailInput) -> Result<(Alerter, Value), AppError> {
    let (_, a) = inventory::resolve_alerter(client, target).await?;
    let raw = client
        .read_references(GetAlerter {
            alerter: a.id.clone(),
        })
        .await?;
    if raw.to_string().len() > 64 * 1024 {
        return Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None));
    }
    let c = &raw["config"];
    if raw["_id"]["$oid"] != a.id
        || raw["name"] != a.name
        || raw["template"] != a.template
        || [
            "enabled",
            "endpoint",
            "alert_types",
            "resources",
            "except_resources",
            "maintenance_windows",
        ]
        .iter()
        .any(|k| c.get(k).is_none())
        || !c["enabled"].is_boolean()
        || endpoint_kind(&c["endpoint"]).is_none()
        || c["endpoint"]["params"]["url"]
            .as_str()
            .is_none_or(str::is_empty)
        || serde_json::from_value::<AlerterConfig>(c.clone()).is_err()
    {
        return Err(invalid());
    }
    Ok((a, raw))
}
pub async fn get(client: &CoreClient, input: DetailInput) -> Result<Detail, AppError> {
    let (a, raw) = snapshot(&client.bounded_evidence(), input).await?;
    let d = summary(&raw["config"])?;
    Ok(Detail {
        summary: Summary {
            id: a.id,
            name: common::bounded_text(client, &a.name, 128),
            template: a.template,
            enabled: d.enabled,
            endpoint_type: d.endpoint_type.clone(),
        },
        definition: d,
    })
}
async fn prepare(
    client: &CoreClient,
    p: &Patch,
    current: Option<&Value>,
) -> Result<(PartialAlerterConfig, Value, Value), AppError> {
    common::validate_payload(client, p)?;
    let mut value = json!({});
    let mut refs = Vec::new();
    if let Some(b) = p.enabled {
        value["enabled"] = json!(b);
    } else if current.is_none() {
        value["enabled"] = json!(false);
    }
    if let Some(e) = &p.endpoint {
        value["endpoint"] = e.prepare(current.map(|c| &c["endpoint"]))?;
    } else if current.is_none() {
        return Err(args(
            "Creation requires an explicit typed endpoint and original URL/reference; no SDK endpoint default is used",
        ));
    }
    if let Some(types) = &p.alert_types {
        if types.len() > 32 {
            return Err(args("Use at most 32 alert types"));
        }
        value["alert_types"] = json!(types);
    }
    for (key, filter) in [
        ("resources", &p.resources),
        ("except_resources", &p.except_resources),
    ] {
        if let Some(filter) = filter {
            if filter.len() > 16 {
                return Err(args("Use at most 16 exact resource filters per list"));
            }
            let mut result = Vec::new();
            for f in filter {
                let target = match f {
                    FilterTarget::System {} => json!({"type":"System","id":"system"}),
                    FilterTarget::Resource {
                        resource_kind,
                        target,
                    } => {
                        let selected = crate::export::select(
                            client,
                            crate::export::Target {
                                kind: *resource_kind,
                                target: target.clone(),
                            },
                        )
                        .await?;
                        let kind = match resource_kind {
                            crate::export::Kind::Server => "Server",
                            crate::export::Kind::Swarm => "Swarm",
                            crate::export::Kind::Stack => "Stack",
                            crate::export::Kind::Deployment => "Deployment",
                            crate::export::Kind::Build => "Build",
                            crate::export::Kind::Repo => "Repo",
                            crate::export::Kind::Builder => "Builder",
                            crate::export::Kind::Alerter => "Alerter",
                            crate::export::Kind::Procedure => "Procedure",
                            crate::export::Kind::Action => "Action",
                            crate::export::Kind::ResourceSync => "ResourceSync",
                        };
                        refs.push(json!({"kind":kind,"id":selected.id,"name":selected.name}));
                        json!({"type":kind,"id":selected.id})
                    }
                };
                if result.contains(&target) {
                    return Err(args("Resource filters must not contain duplicates"));
                }
                result.push(target);
            }
            value[key] = json!(result);
        }
    }
    if let Some(w) = &p.maintenance_windows {
        if w.len() > 16 {
            return Err(args("Use at most 16 maintenance windows"));
        }
        value["maintenance_windows"] = json!(
            w.iter()
                .map(MaintenanceWindow::value)
                .collect::<Result<Vec<_>, _>>()?
        );
    }
    let typed = serde_json::from_value(value.clone())
        .map_err(|_| args("Use supported typed Alerter fields"))?;
    // Detect SDK coercion/defaults rather than silently rewriting supplied data.
    if common::partial_value(&typed)? != value {
        return Err(invalid());
    }
    Ok((typed, value, json!(refs)))
}
fn proposed(before: Option<&Value>, patch: &Value) -> Value {
    let mut c = before
        .cloned()
        .unwrap_or_else(|| serde_json::to_value(AlerterConfig::default()).expect("config"));
    for (k, v) in patch.as_object().expect("object") {
        c[k] = v.clone();
    }
    c
}
fn result(
    client: &CoreClient,
    id: Option<String>,
    name: &str,
    before: Option<&Value>,
    patch: &Value,
) -> Result<ConfigurationResult, AppError> {
    let after = proposed(before, patch);
    // Changes disclose field identity only; never serialize raw values or lengths.
    let changes = patch
        .as_object()
        .expect("patch")
        .iter()
        .filter(|(k, v)| before.is_none_or(|b| b.get(k.as_str()) != Some(*v)))
        .map(|(k, _)| FieldChange {
            field: k.clone(),
            before: json!("omitted"),
            after: json!("supplied"),
            redacted: true,
            before_truncated: false,
            after_truncated: false,
        })
        .collect();
    Ok(ConfigurationResult {
        outcome: ConfigurationOutcome::Preview,
        resource: ConfigurationTarget {
            kind: "alerter",
            id,
            name: common::bounded_text(client, name, 128),
        },
        preview_id: None,
        changes,
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects: vec![
            EFFECT,
            DELIVERY_LIMIT,
            "Private ten-minute single-use previews bind the entire raw resource/configuration, including secret/reference-only changes. Apply checks again before one write and independently reads the complete saved configuration. Core offers no CAS; a final-read/write race remains. Unknown replies are never retried.",
        ],
        definition: Some(common::DefinitionPreview::Alerter(DefinitionPreview {
            before: before.map(summary).transpose()?,
            after: summary(&after)?,
            requires_operate_to_apply: before.is_some() || patch["enabled"] == true,
        })),
    })
}
fn mutable(a: &Alerter) -> Result<(), AppError> {
    if a.template {
        Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None))
    } else {
        Ok(())
    }
}
fn baseline(raw: &Value) -> Value {
    raw.clone()
}
async fn admin(client: &CoreClient) -> Result<Value, AppError> {
    let u = client.current_caller().await?;
    if u["enabled"] != true || u["admin"] != true {
        return Err(AppError {
            correction: Some(
                "Core requires an enabled administrator to create an Alerter. Never switch credentials after denial.",
            ),
            ..AppError::new(ErrorKind::PermissionDenied, "CreateAlerter", None)
        });
    }
    Ok(u)
}
fn require_operate_if(
    client: &CoreClient,
    required: bool,
    op: &'static str,
) -> Result<(), AppError> {
    if required {
        operations::require_operate(client, op)
    } else {
        Ok(())
    }
}
pub(crate) async fn guard_write(
    client: &CoreClient,
    op: &'static str,
    p: &Value,
) -> Result<(), AppError> {
    if !matches!(op, "CreateAlerter" | "UpdateAlerter") {
        return Ok(());
    }
    common::require_edit(client)?;
    require_operate_if(
        client,
        op == "UpdateAlerter" || p["config"]["enabled"] == true,
        op,
    )?;
    if op == "UpdateAlerter"
        && p["id"]
            .as_str()
            .is_none_or(|s| !crate::deletion::object_id(s))
    {
        return Err(args("UpdateAlerter requires one stable ID"));
    }
    if op == "CreateAlerter" {
        admin(client).await?;
    }
    if op == "CreateAlerter"
        && (!p["config"]["enabled"].is_boolean() || p["config"].get("endpoint").is_none())
    {
        return Err(args(
            "Creation requires explicit enabled state and endpoint",
        ));
    }
    Ok(())
}
pub async fn preview_update(
    client: &CoreClient,
    input: PreviewInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    common::validate_payload(client, &input)?;
    let b = client.bounded_evidence();
    let (a, raw) = snapshot(&b, input.target.clone()).await?;
    mutable(&a)?;
    let (_, patch, refs) = prepare(&b, &input.patch, Some(&raw["config"])).await?;
    let r = result(client, Some(a.id), &a.name, Some(&raw["config"]), &patch)?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("alerter_update", &input),
        baseline: baseline(&raw),
        prepared: json!({"config":patch,"references":refs}),
        result: r,
    })
}
pub async fn apply_update(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    operations::require_operate(client, "UpdateAlerter")?;
    let submitted = PreviewInput {
        target: input.target.clone(),
        patch: input.patch.clone(),
    };
    let mut e = client.previews().take(
        &input.preview_id,
        &common::bound_input("alerter_update", &submitted),
    )?;
    let b = client.bounded_evidence();
    let (a, raw) = snapshot(&b, input.target).await?;
    mutable(&a)?;
    let (typed, patch, refs) = prepare(&b, &input.patch, Some(&raw["config"])).await?;
    if e.baseline != baseline(&raw) || e.prepared != json!({"config":patch,"references":refs}) {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    e.result.preview_id = None;
    if e.result.changes.is_empty() {
        e.result.outcome = ConfigurationOutcome::NoOp;
        e.result.saved_configuration_verified = true;
        return Ok(e.result);
    }
    let expected = proposed(Some(&raw["config"]), &patch);
    let response = client
        .write_checked(UpdateAlerter {
            id: a.id.clone(),
            config: typed,
        })
        .await?;
    Ok(finish(
        client,
        e.result,
        response,
        &patch,
        &expected,
        &a.name,
        Some(&a.id),
        "UpdateAlerter",
    )
    .await)
}
fn create_name(client: &CoreClient, input: &CreateInput) -> Result<String, AppError> {
    common::validate_payload(client, input)?;
    let name = komodo_client::entities::to_general_name(&input.name);
    if input.name.len() > 256
        || input.name.chars().any(char::is_control)
        || name.is_empty()
        || name.len() > 128
        || crate::deletion::object_id(&name)
    {
        return Err(args("Use a nonempty bounded Alerter name"));
    }
    Ok(name)
}
async fn absent(client: &CoreClient, name: &str) -> Result<(), AppError> {
    match inventory::resolve_alerter(
        client,
        DetailInput {
            id: None,
            name: Some(name.into()),
        },
    )
    .await
    {
        Err(e) if e.kind == ErrorKind::NotFound => Ok(()),
        Ok(_) => Err(AppError::new(ErrorKind::StalePreview, OP, None)),
        Err(e) => Err(e),
    }
}
pub async fn preview_create(
    client: &CoreClient,
    input: CreateInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let name = create_name(client, &input)?;
    let u = admin(client).await?;
    let b = client.bounded_evidence();
    absent(&b, &name).await?;
    let (_, patch, refs) = prepare(&b, &input.config, None).await?;
    let r = result(client, None, &name, None, &patch)?;
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("alerter_create", &input),
        baseline: json!({"name":name,"caller":u}),
        prepared: json!({"config":patch,"references":refs}),
        result: r,
    })
}
pub async fn apply_create(
    client: &CoreClient,
    input: CreateApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    require_operate_if(client, input.config.enabled == Some(true), "CreateAlerter")?;
    let submitted = CreateInput {
        name: input.name,
        config: input.config,
    };
    let name = create_name(client, &submitted)?;
    let mut e = client.previews().take(
        &input.preview_id,
        &common::bound_input("alerter_create", &submitted),
    )?;
    let u = admin(client).await?;
    let b = client.bounded_evidence();
    absent(&b, &name).await?;
    let (typed, patch, refs) = prepare(&b, &submitted.config, None).await?;
    if e.baseline != json!({"name":name,"caller":u})
        || e.prepared != json!({"config":patch,"references":refs})
    {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    e.result.preview_id = None;
    let expected = proposed(None, &patch);
    let response = client
        .write_checked(CreateAlerter {
            name: name.clone(),
            config: typed,
        })
        .await?;
    Ok(finish(
        client,
        e.result,
        response,
        &patch,
        &expected,
        &name,
        None,
        "CreateAlerter",
    )
    .await)
}
#[allow(clippy::too_many_arguments)]
async fn finish(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<Alerter, AppError>,
    patch: &Value,
    expected: &Value,
    name: &str,
    id: Option<&str>,
    op: &'static str,
) -> ConfigurationResult {
    let mut r = common::finish_save(r, response, patch, name, id, op, |_, _| {});
    if matches!(r.outcome, ConfigurationOutcome::Saved) {
        let read = client
            .bounded_evidence()
            .read_references(GetAlerter {
                alerter: r.resource.id.clone().expect("saved"),
            })
            .await;
        r = common::verify_readback(r, read, expected, name, "GetAlerter");
    }
    r
}
pub(crate) async fn guard_execute(
    client: &CoreClient,
    op: &'static str,
    p: &Value,
) -> Result<(), AppError> {
    if op != "TestAlerter" {
        return Ok(());
    }
    operations::require_operate(client, op)?;
    let id = p["alerter"]
        .as_str()
        .filter(|s| crate::deletion::object_id(s))
        .ok_or_else(|| args("Test one saved Alerter by stable ID"))?;
    if p.as_object().is_none_or(|m| m.len() != 1) {
        return Err(args("TestAlerter accepts only its exact saved target"));
    }
    let (a, _) = snapshot(
        &client.bounded_evidence(),
        DetailInput {
            id: Some(id.into()),
            name: None,
        },
    )
    .await?;
    mutable(&a)
}
pub async fn test(client: &CoreClient, input: TestInput) -> Result<TestResult, AppError> {
    operations::require_operate(client, "TestAlerter")?;
    if !input.acknowledge_delivery {
        return Err(args(
            "Explicitly acknowledge one test delivery to this saved endpoint; never use a real recipient without authorization",
        ));
    }
    let (a, _) = snapshot(&client.bounded_evidence(), input.target).await?;
    mutable(&a)?;
    let r = operations::submit(
        client,
        OperationResource {
            kind: WorkloadKind::Alerter,
            id: a.id.clone(),
            name: common::bounded_text(client, &a.name, 128),
        },
        RequestedScope::AlerterTest {},
        WorkloadOperation::Test,
        Operation::TestAlerter,
        ConfiguredEffects {
            pre_deploy_hook: false,
            post_deploy_hook: false,
            auto_pull: None,
            saved_extra_args: false,
            saved_command_wrapper: false,
            broader_effects_possible: true,
            compose_dependencies: None,
            repository: None,
            build: None,
            procedure: None,
            action: None,
        },
        TestAlerter { alerter: a.id },
    )
    .await?;
    Ok(TestResult {
        receipt: r,
        delivery_limit: DELIVERY_LIMIT,
    })
}
pub(crate) fn private_history(op: Operation) -> bool {
    matches!(
        op,
        Operation::CreateAlerter
            | Operation::UpdateAlerter
            | Operation::RenameAlerter
            | Operation::DeleteAlerter
            | Operation::TestAlerter
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{capabilities::CapabilityPolicy, client::RequestPolicy, config::Config};
    fn client(operate: bool, edit: bool) -> CoreClient {
        CoreClient::new(
            Config::parse("http://127.0.0.1:1", "fixture-key", "fixture-secret")
                .unwrap()
                .with_capabilities(CapabilityPolicy::new(operate, edit)),
            RequestPolicy::default(),
        )
        .unwrap()
    }
    #[tokio::test]
    async fn shared_transport_denies_disabled_mutations_and_broadcast_escape_before_network() {
        for operate in [false, true] {
            for edit in [false, true] {
                let c = client(operate, edit);
                if !edit || !operate {
                    let e = c
                        .write_checked(UpdateAlerter {
                            id: "000000000000000000000081".into(),
                            config: PartialAlerterConfig {
                                enabled: Some(true),
                                ..Default::default()
                            },
                        })
                        .await
                        .unwrap_err();
                    assert_eq!(e.kind, ErrorKind::CapabilityDisabled);
                    let e = c
                        .write_checked(CreateAlerter {
                            name: "new".into(),
                            config: PartialAlerterConfig {
                                enabled: Some(true),
                                endpoint: Some(Default::default()),
                                ..Default::default()
                            },
                        })
                        .await
                        .unwrap_err();
                    assert_eq!(e.kind, ErrorKind::CapabilityDisabled);
                }
                if !operate {
                    let e = c
                        .execute(TestAlerter {
                            alerter: "000000000000000000000081".into(),
                        })
                        .await
                        .unwrap_err();
                    assert_eq!(e.kind, ErrorKind::CapabilityDisabled);
                }
                let p = json!({"alerter":"name-instead-of-id"});
                assert_eq!(
                    guard_execute(&c, "TestAlerter", &p).await.unwrap_err().kind,
                    if operate {
                        ErrorKind::InvalidArguments
                    } else {
                        ErrorKind::CapabilityDisabled
                    }
                );
                assert_eq!(
                    c.execute(komodo_client::api::execute::SendAlert {
                        message: "no broadcast".into(),
                        level: Default::default(),
                        details: String::new(),
                        alerters: Vec::new(),
                    })
                    .await
                    .unwrap_err()
                    .kind,
                    ErrorKind::UnsupportedTarget
                );
            }
        }
    }
    #[test]
    fn maintenance_dates_and_closed_types_reject_malformed_input() {
        assert!(valid_date("2024-02-29"));
        assert!(!valid_date("2025-02-29"));
        assert!(!valid_date("2026-13-01"));
        assert!(!valid_date("2026-00-01"));
        assert!(!valid_date("0000-01-01"));
        assert!(!valid_date("ééééé"));
        assert!(
            serde_json::from_value::<Patch>(json!({"endpoint":{"kind":"ntfy","email":null}}))
                .is_err()
        );
        assert!(
            serde_json::from_value::<EndpointPatch>(
                json!({"kind":"custom","url":"x","token":"unsupported"})
            )
            .is_err()
        );
    }
}
