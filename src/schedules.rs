//! Core-owned scheduling. MCP saves typed owner fields and reports Core cache
//! evidence; it never parses schedules or runs a background scheduler.
use crate::{
    client::CoreClient,
    configuration::{
        self as common, ConfigurationOutcome, ConfigurationResult, ConfigurationTarget, Preview,
        present,
    },
    error::{AppError, ErrorKind},
    inventory::{self, DetailInput, ListInput, Page},
};
use komodo_client::{
    api::{
        read::{GetAction, GetProcedure, ListSchedules},
        write::{UpdateAction, UpdateProcedure},
    },
    entities::{
        ResourceTarget, ScheduleFormat,
        action::{Action, PartialActionConfig},
        procedure::{PartialProcedureConfig, Procedure},
        resource::Resource,
        schedule::{Schedule, ScheduleSortBy},
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::time::Instant;
const OP: &str = "schedule_configuration";
const OBSERVATION_PAGES: u64 = 4;
pub const LIMIT: &str = "Core lists Action/Procedure owners with nonempty expressions, including disabled owners and templates. MCP requests bounded positive pages; Core internally collects/sorts all matching owners before pagination. Next-run/error values come from Core's scheduler cache, with no cache generation or atomic configuration link. Missing/cached values are not proof of validity or execution. Disabled/empty expressions are not parsed. Last run is the start time of the latest completed run, successful or failed, manual or scheduled; it gives no caller/scheduled attribution or workload health.";
pub const EFFECT: &str = "Schedules execute trusted saved automation using Core's built-in administrator Action/Procedure identities; Actions also have Core host Deno --allow-all access. Every actual schedule write requires both local grants and Core owner Write. Disabling/clearing does not cancel already dispatched/running work. In-flight Core execution can reschedule using its previously captured definition, racing edits. Inspect Core cache separately; no CAS, rollback, automatic retry or MCP scheduler is provided.";
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum OwnerKind {
    Action,
    Procedure,
}
impl OwnerKind {
    fn name(self) -> &'static str {
        match self {
            Self::Action => "action",
            Self::Procedure => "procedure",
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Format {
    Cron,
    English,
}
impl From<ScheduleFormat> for Format {
    fn from(f: ScheduleFormat) -> Self {
        match f {
            ScheduleFormat::Cron => Self::Cron,
            ScheduleFormat::English => Self::English,
        }
    }
}
impl From<Format> for ScheduleFormat {
    fn from(f: Format) -> Self {
        match f {
            Format::Cron => Self::Cron,
            Format::English => Self::English,
        }
    }
}
#[derive(Clone, Default, Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Patch {
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub expression: Option<String>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "Format")]
    pub format: Option<Format>,
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "present"
    )]
    #[schemars(with = "String")]
    pub timezone: Option<String>,
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
    #[schemars(with = "bool")]
    pub alert: Option<bool>,
}
impl Patch {
    fn value(&self, client: &CoreClient) -> Result<Value, AppError> {
        common::validate_payload(client, self)?;
        if self
            .expression
            .as_ref()
            .is_some_and(|s| s.len() > 1024 || s.chars().any(char::is_control))
            || self
                .timezone
                .as_ref()
                .is_some_and(|s| s.len() > 128 || s.chars().any(char::is_control))
        {
            return Err(AppError::arguments(
                OP,
                "Use an expression of at most 1024 UTF-8 bytes and a timezone of at most 128 bytes, without control characters. Core parses Cron/English/timezones authoritatively; an empty timezone inherits Core defaults. Empty expression deliberately clears the schedule.",
            ));
        }
        // Both owner partials serialize these five fields identically. No source,
        // stages, arguments, startup, webhook or secret field can be supplied.
        common::partial_value(&PartialActionConfig {
            schedule: self.expression.clone(),
            schedule_format: self.format.map(Into::into),
            schedule_timezone: self.timezone.clone(),
            schedule_enabled: self.enabled,
            schedule_alert: self.alert,
            ..Default::default()
        })
    }
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct DetailRequest {
    pub owner_kind: OwnerKind,
    pub target: DetailInput,
}
#[derive(Serialize, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct PreviewInput {
    pub owner_kind: OwnerKind,
    pub target: DetailInput,
    pub patch: Patch,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ApplyInput {
    pub owner_kind: OwnerKind,
    pub target: DetailInput,
    pub patch: Patch,
    pub preview_id: String,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Owner {
    pub kind: OwnerKind,
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Fields {
    pub expression: String,
    pub expression_truncated: bool,
    pub format: Format,
    pub timezone: String,
    pub timezone_truncated: bool,
    pub timezone_source: &'static str,
    pub enabled: bool,
    pub alert: Option<bool>,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ScheduleSummary {
    pub owner: Owner,
    pub fields: Fields,
    pub next_scheduled_run: Option<i64>,
    pub last_run_at: Option<i64>,
    pub schedule_error: Option<String>,
    pub schedule_error_truncated: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct SchedulePage {
    #[serde(flatten)]
    pub page: Page<ScheduleSummary>,
    pub evidence_limit: &'static str,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ObservationStatus {
    Reported,
    NotListedEmptyExpression,
    NotFound,
    SearchLimit,
    Unavailable,
    ConfigurationMismatch,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct Observation {
    pub status: ObservationStatus,
    pub reported: Option<ScheduleSummary>,
    pub matches_expected_fields: Option<bool>,
    pub pages_read: u8,
    pub error: Option<AppError>,
    pub evidence_limit: &'static str,
}
#[derive(Serialize, JsonSchema)]
pub struct Detail {
    pub owner: Owner,
    pub template: bool,
    pub saved: Fields,
    pub observation: Observation,
    pub effects: &'static str,
}
#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DefinitionPreview {
    pub before: Fields,
    pub after: Fields,
    pub requires_operate_to_apply: bool,
    pub observation: Observation,
}
fn text(client: &CoreClient, s: &str, max: usize) -> (String, bool) {
    let value = common::bounded_text(client, &crate::logs::redact_log(client, s), max);
    let truncated = value != s;
    (value, truncated)
}
fn fields(client: &CoreClient, config: &Value) -> Fields {
    let expression = config["schedule"].as_str().expect("typed schedule");
    let timezone = config["schedule_timezone"]
        .as_str()
        .expect("typed timezone");
    let (expression, expression_truncated) = text(client, expression, 1024);
    let (timezone, timezone_truncated) = text(client, timezone, 128);
    Fields {
        expression,
        expression_truncated,
        format: if config["schedule_format"] == "Cron" {
            Format::Cron
        } else {
            Format::English
        },
        timezone,
        timezone_truncated,
        timezone_source: if config["schedule_timezone"] == "" {
            "core_configured_timezone_or_core_host_local"
        } else {
            "explicit"
        },
        enabled: config["schedule_enabled"].as_bool().expect("typed enabled"),
        alert: config["schedule_alert"].as_bool(),
    }
}
fn id(s: &Schedule) -> &str {
    match &s.target {
        ResourceTarget::Action(id) | ResourceTarget::Procedure(id) => id,
        _ => "",
    }
}
fn owner(s: &Schedule) -> Result<OwnerKind, AppError> {
    if !crate::deletion::object_id(id(s))
        || s.name.trim().is_empty()
        || s.schedule.is_empty()
        || s.next_scheduled_run.is_some_and(|t| t < 0)
        || s.last_run_at.is_some_and(|t| t < 0)
        || s.next_scheduled_run.is_some() && s.schedule_error.is_some()
    {
        return Err(invalid());
    }
    match s.target {
        ResourceTarget::Action(_) => Ok(OwnerKind::Action),
        ResourceTarget::Procedure(_) => Ok(OwnerKind::Procedure),
        _ => Err(invalid()),
    }
}
fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, "ListSchedules", None)
}
fn summary(client: &CoreClient, s: &Schedule) -> Result<ScheduleSummary, AppError> {
    let kind = owner(s)?;
    let error = s.schedule_error.as_ref().map(|s| text(client, s, 512));
    Ok(ScheduleSummary {
        owner: Owner {
            kind,
            id: id(s).to_ascii_lowercase(),
            name: common::bounded_text(client, &s.name, 256),
        },
        fields: fields(
            client,
            &json!({"schedule":s.schedule,"schedule_format":s.schedule_format,"schedule_timezone":s.schedule_timezone,"schedule_enabled":s.enabled}),
        ),
        next_scheduled_run: s.next_scheduled_run,
        last_run_at: s.last_run_at,
        schedule_error: error.as_ref().map(|(s, _)| s.clone()),
        schedule_error_truncated: error.is_some_and(|(_, t)| t),
    })
}
async fn page(
    client: &CoreClient,
    p: u64,
    limit: u64,
    terms: &[String],
) -> Result<Vec<Schedule>, AppError> {
    let values = client
        .read(ListSchedules {
            page: p,
            limit: Some(limit),
            terms: terms.to_vec(),
            sort_by: ScheduleSortBy::Name,
            sort_desc: false,
            ..Default::default()
        })
        .await?;
    if values.len() as u64 > limit {
        return Err(invalid());
    }
    let mut seen = std::collections::BTreeSet::new();
    for s in &values {
        let k = owner(s)?;
        if !seen.insert((k.name(), id(s))) {
            return Err(invalid());
        }
    }
    Ok(values)
}
pub async fn list(client: &CoreClient, input: ListInput) -> Result<SchedulePage, AppError> {
    let bounded = client.bounded_evidence();
    let p = inventory::paginate(input, "ListSchedules", |p, l| page(&bounded, p, l, &[])).await?;
    Ok(SchedulePage {
        page: Page {
            items: p
                .items
                .iter()
                .map(|s| summary(client, s))
                .collect::<Result<_, _>>()?,
            page: p.page,
            page_size: p.page_size,
            has_more: p.has_more,
            next_page: p.next_page,
        },
        evidence_limit: LIMIT,
    })
}
enum Parent {
    Action(Box<Action>),
    Procedure(Box<Procedure>),
}
impl Parent {
    async fn resolve(
        client: &CoreClient,
        kind: OwnerKind,
        target: DetailInput,
    ) -> Result<Self, AppError> {
        Ok(match kind {
            OwnerKind::Action => {
                Self::Action(Box::new(inventory::resolve_action(client, target).await?.1))
            }
            OwnerKind::Procedure => Self::Procedure(Box::new(
                inventory::resolve_procedure(client, target).await?.1,
            )),
        })
    }
    fn kind(&self) -> OwnerKind {
        match self {
            Self::Action(_) => OwnerKind::Action,
            Self::Procedure(_) => OwnerKind::Procedure,
        }
    }
    fn id(&self) -> &str {
        match self {
            Self::Action(p) => &p.id,
            Self::Procedure(p) => &p.id,
        }
    }
    fn name(&self) -> &str {
        match self {
            Self::Action(p) => &p.name,
            Self::Procedure(p) => &p.name,
        }
    }
    fn template(&self) -> bool {
        match self {
            Self::Action(p) => p.template,
            Self::Procedure(p) => p.template,
        }
    }
    fn config(&self) -> Value {
        match self {
            Self::Action(p) => serde_json::to_value(&p.config),
            Self::Procedure(p) => serde_json::to_value(&p.config),
        }
        .expect("typed parent")
    }
    fn baseline(&self) -> Value {
        match self {
            Self::Action(p) => common::resource_baseline("action_schedule", p),
            Self::Procedure(p) => common::resource_baseline("procedure_schedule", p),
        }
    }
    fn public_owner(&self, client: &CoreClient) -> Owner {
        Owner {
            kind: self.kind(),
            id: self.id().into(),
            name: common::bounded_text(client, self.name(), 256),
        }
    }
    fn writable(&self) -> Result<(), AppError> {
        if self.template() {
            Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None))
        } else {
            Ok(())
        }
    }
    async fn raw(&self, client: &CoreClient) -> Result<Value, AppError> {
        match self {
            Self::Action(p) => {
                client
                    .read_references(GetAction {
                        action: p.id.clone(),
                    })
                    .await
            }
            Self::Procedure(p) => {
                client
                    .read_references(GetProcedure {
                        procedure: p.id.clone(),
                    })
                    .await
            }
        }
    }
}
fn observation(status: ObservationStatus, pages: u8) -> Observation {
    Observation {
        status,
        reported: None,
        matches_expected_fields: None,
        pages_read: pages,
        error: None,
        evidence_limit: LIMIT,
    }
}
async fn observe(client: &CoreClient, p: &Parent, config: &Value) -> Observation {
    if config["schedule"] == "" {
        return observation(ObservationStatus::NotListedEmptyExpression, 0);
    }
    let terms = vec![p.name().to_owned()];
    let mut pages = 0;
    for i in 0..OBSERVATION_PAGES {
        pages += 1;
        let values = match page(client, i, 100, &terms).await {
            Ok(v) => v,
            Err(e) => {
                let mut o = observation(ObservationStatus::Unavailable, pages);
                o.error = Some(e);
                return o;
            }
        };
        if let Some(s) = values
            .iter()
            .find(|s| id(s) == p.id() && owner(s).ok() == Some(p.kind()))
        {
            let matches = s.name == p.name()
                && s.schedule == config["schedule"]
                && serde_json::to_value(s.schedule_format).expect("format")
                    == config["schedule_format"]
                && s.schedule_timezone == config["schedule_timezone"]
                && s.enabled == config["schedule_enabled"];
            return Observation {
                status: if matches {
                    ObservationStatus::Reported
                } else {
                    ObservationStatus::ConfigurationMismatch
                },
                reported: Some(summary(client, s).expect("validated page")),
                matches_expected_fields: Some(matches),
                pages_read: pages,
                error: None,
                evidence_limit: LIMIT,
            };
        }
        if values.len() < 100 {
            return observation(ObservationStatus::NotFound, pages);
        }
    }
    observation(ObservationStatus::SearchLimit, pages)
}
pub async fn get(client: &CoreClient, input: DetailRequest) -> Result<Detail, AppError> {
    let bounded = client.bounded_evidence();
    let p = Parent::resolve(&bounded, input.owner_kind, input.target).await?;
    let c = p.config();
    Ok(Detail {
        owner: p.public_owner(client),
        template: p.template(),
        saved: fields(client, &c),
        observation: observe(&bounded, &p, &c).await,
        effects: EFFECT,
    })
}
fn after(p: &Parent, patch: &Value) -> Value {
    let mut c = p.config();
    for (k, v) in patch.as_object().expect("partial") {
        c[k] = v.clone();
    }
    c
}
pub async fn preview(
    client: &CoreClient,
    input: PreviewInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    let patch = input.patch.value(client)?;
    let bounded = client.bounded_evidence();
    let p = Parent::resolve(&bounded, input.owner_kind, input.target.clone()).await?;
    p.writable()?;
    let before = p.config();
    let after = after(&p, &patch);
    let changes = common::changes(client, &before, &patch);
    let r = ConfigurationResult {
        outcome: if changes.is_empty() {
            ConfigurationOutcome::NoOp
        } else {
            ConfigurationOutcome::Preview
        },
        resource: ConfigurationTarget {
            kind: p.kind().name(),
            id: Some(p.id().into()),
            name: common::bounded_text(client, p.name(), 256),
        },
        preview_id: None,
        changes,
        writes_submitted: 0,
        saved_configuration_verified: false,
        observation_error: None,
        effects: vec![
            EFFECT,
            LIMIT,
            "Omitted schedule fields and the complete parent source/stages/default arguments/secrets/unrelated settings are preserved. Empty expression clears the schedule; empty timezone inherits Core defaults. A verified save does not prove parsing, a fresh cache generation or execution. Core parse errors are observed separately; MCP computes no next run.",
        ],
        definition: Some(common::DefinitionPreview::Schedule(Box::new(
            DefinitionPreview {
                before: fields(client, &before),
                after: fields(client, &after),
                requires_operate_to_apply: !patch.as_object().expect("partial").is_empty(),
                observation: observe(&bounded, &p, &before).await,
            },
        ))),
    };
    client.previews().insert(Preview {
        created: Instant::now(),
        input: common::bound_input("schedule_update", &input),
        baseline: p.baseline(),
        prepared: patch,
        result: r,
    })
}
fn echo<C: Default + Serialize, I: Default>(
    client: &CoreClient,
    r: ConfigurationResult,
    response: Result<Resource<C, I>, AppError>,
    patch: &Value,
    p: &Parent,
    op: &'static str,
) -> ConfigurationResult {
    common::finish_save(r, response, patch, p.name(), Some(p.id()), op, |r, c| {
        if let Some(common::DefinitionPreview::Schedule(d)) = &mut r.definition {
            d.after = fields(client, &serde_json::to_value(c).expect("config"));
        }
    })
}
pub async fn apply(
    client: &CoreClient,
    input: ApplyInput,
) -> Result<ConfigurationResult, AppError> {
    common::require_edit(client)?;
    crate::builders::require_operate(
        client,
        !common::partial_value(&input.patch)?
            .as_object()
            .expect("patch")
            .is_empty(),
        OP,
    )?;
    let submitted = PreviewInput {
        owner_kind: input.owner_kind,
        target: input.target.clone(),
        patch: input.patch,
    };
    let entry = client.previews().take(
        &input.preview_id,
        &common::bound_input("schedule_update", &submitted),
    )?;
    let bounded = client.bounded_evidence();
    let p = Parent::resolve(&bounded, input.owner_kind, input.target).await?;
    p.writable()?;
    if p.baseline() != entry.baseline {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let patch = submitted.patch.value(client)?;
    if patch != entry.prepared {
        return Err(AppError::new(ErrorKind::StalePreview, OP, None));
    }
    let after = after(&p, &patch);
    let mut r = entry.result;
    r.preview_id = None;
    if r.changes.is_empty() {
        r.outcome = ConfigurationOutcome::NoOp;
        r.saved_configuration_verified = true;
    } else {
        r = match &p {
            Parent::Action(a) => echo(
                client,
                r,
                client
                    .write_checked(UpdateAction {
                        id: a.id.clone(),
                        config: serde_json::from_value(patch.clone())
                            .expect("typed action schedule partial"),
                    })
                    .await?,
                &patch,
                &p,
                "UpdateAction",
            ),
            Parent::Procedure(a) => echo(
                client,
                r,
                client
                    .write_checked(UpdateProcedure {
                        id: a.id.clone(),
                        config: serde_json::from_value::<PartialProcedureConfig>(patch.clone())
                            .expect("typed procedure schedule partial"),
                    })
                    .await?,
                &patch,
                &p,
                "UpdateProcedure",
            ),
        };
        if matches!(r.outcome, ConfigurationOutcome::Saved) {
            r = common::verify_readback(
                r,
                p.raw(&bounded).await,
                &after,
                p.name(),
                match p.kind() {
                    OwnerKind::Action => "GetAction",
                    OwnerKind::Procedure => "GetProcedure",
                },
            );
        }
    }
    // A save ACK/read-back and Core's parser/cache are independent evidence.
    // Read failures here never erase the acknowledged save or replay the write.
    if let Some(common::DefinitionPreview::Schedule(d)) = &mut r.definition {
        d.observation = observe(&bounded, &p, &after).await;
    }
    Ok(r)
}
