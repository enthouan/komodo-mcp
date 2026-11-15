//! Bounded, read-only Docker observations on one explicitly selected Server.
use std::collections::BTreeSet;

use komodo_client::{
    api::read::{
        InspectImage, InspectNetwork, InspectVolume, ListImageHistory, ListImages, ListNetworks,
        ListVolumes,
    },
    entities::docker::{
        image::{Image, ImageListItem},
        network::{Network, NetworkListItem},
        volume::{Volume, VolumeListItem},
    },
};
use rmcp::schemars::{self, JsonSchema};
use serde::{Deserialize, Serialize};

use crate::{
    client::CoreClient,
    error::{AppError, Candidate, ErrorKind},
    inventory::{self, DetailInput},
};

const SCAN: usize = 512;
const CACHE: &str = "Core cache; no generation timestamp or completeness guarantee. Empty can mean no objects, missing cache or a suppressed Periphery poll failure. Pagination bounds local output, not Core/Docker enumeration. Cached in_use is not an authoritative prune dry run.";
const FRESH: &str = "Fresh exact-object inspection through the selected Server. Lists and details are separate non-atomic reads. Raw configuration, labels, commands, driver options, mountpoints and credentials are omitted. Volume names have no immutable identity in Core's list: a same-name replacement cannot be excluded.";

fn size() -> u64 {
    30
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ListInput {
    pub target: DetailInput,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u64,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Selector {
    #[serde(default, deserialize_with = "crate::configuration::present")]
    pub id: Option<String>,
    #[serde(default, deserialize_with = "crate::configuration::present")]
    pub name: Option<String>,
}
impl JsonSchema for Selector {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "DockerObjectSelector".into()
    }
    fn json_schema(_: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({"type":"object","properties":{
            "id":{"type":"string","pattern":"^((sha256:)?[0-9a-f]{64}|[a-z0-9]{25})$","description":"Full image sha256: digest or full local/Swarm network ID. Volumes use exact name only."},
            "name":{"type":"string","minLength":1,"maxLength":512,"description":"Exact image reference or network/volume name; no prefixes, credentials or wildcards."}
        },"oneOf":[{"required":["id"]},{"required":["name"]}],"additionalProperties":false})
    }
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ObjectInput {
    pub target: DetailInput,
    pub object: Selector,
}
#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct HistoryInput {
    pub target: DetailInput,
    pub object: Selector,
    #[serde(default)]
    pub page: u64,
    #[serde(default = "size")]
    #[schemars(range(min = 1, max = 100))]
    pub page_size: u64,
}
#[derive(Serialize, JsonSchema)]
pub struct Server {
    pub id: String,
    pub name: String,
    pub cached_state: String,
    pub enabled: bool,
    pub infrastructure_error_present: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct Observation<T> {
    pub server: Server,
    pub source: &'static str,
    pub evidence_limit: &'static str,
    pub data: T,
}
#[derive(Serialize, JsonSchema)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub next_page: Option<u64>,
    pub observed_count: usize,
    pub scan_incomplete: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct ImageSummary {
    pub id: String,
    pub tags: Vec<String>,
    pub digests: Vec<String>,
    pub references_incomplete: bool,
    pub created_unix_seconds: i64,
    pub size_bytes: Option<i64>,
    pub cached_in_use: bool,
}
#[derive(Serialize, JsonSchema)]
pub struct ImageDetail {
    pub id: String,
    pub tags: Vec<String>,
    pub digests: Vec<String>,
    pub references_incomplete: bool,
    pub created: Option<String>,
    pub architecture: Option<String>,
    pub os: Option<String>,
    pub size_bytes: Option<i64>,
    pub rootfs_layer_count: Option<usize>,
}
#[derive(Serialize, JsonSchema)]
pub struct ImageHistory {
    pub image_id: String,
    pub layers: Page<Layer>,
}
#[derive(Serialize, JsonSchema)]
pub struct Layer {
    pub index: usize,
    pub id: Option<String>,
    pub created_unix_seconds: i64,
    pub size_bytes: Option<i64>,
}
#[derive(Serialize, JsonSchema)]
pub struct NetworkSummary {
    pub id: String,
    pub name: Option<String>,
    pub driver: Option<String>,
    pub scope: Option<String>,
    pub created: Option<String>,
    pub enable_ipv6: Option<bool>,
    pub internal: Option<bool>,
    pub attachable: Option<bool>,
    pub ingress: Option<bool>,
    pub cached_in_use: Option<bool>,
    pub attached_container_count: Option<usize>,
}
#[derive(Serialize, JsonSchema)]
pub struct VolumeSummary {
    /// Docker volumes have names, not stable immutable object IDs in Core lists.
    pub name: String,
    pub driver: String,
    pub scope: String,
    pub created: Option<String>,
    pub size_bytes: Option<i64>,
    pub cached_in_use: Option<bool>,
    pub reference_count: Option<i64>,
}
#[derive(Clone, Copy)]
enum Kind {
    Image,
    Network,
    Volume,
}
fn invalid() -> AppError {
    AppError::new(ErrorKind::InvalidResponse, "docker_observation", None)
}
fn hex(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn network_id(v: &str) -> bool {
    hex(v)
        || (v.len() == 25
            && v.bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
}
fn image_id(v: &str) -> bool {
    v.strip_prefix("sha256:").is_some_and(hex)
}
fn name(v: &str) -> bool {
    !v.is_empty()
        && v.len() <= 128
        && v.as_bytes()[0].is_ascii_alphanumeric()
        && v.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}
fn image_reference(v: &str) -> bool {
    if v.is_empty()
        || v.len() > 512
        || !v.as_bytes()[0].is_ascii_alphanumeric()
        || v.contains("://")
        || !v
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-/:@".contains(&b))
    {
        return false;
    }
    let mut parts = v.split('@');
    let path = parts.next().unwrap_or_default();
    if path.ends_with(['/', ':'])
        || path
            .split('/')
            .any(|p| p.is_empty() || p == "." || p == "..")
    {
        return false;
    }
    // The only permitted @ suffix is a manifest digest, never URL userinfo.
    parts.next().is_none_or(image_id) && parts.next().is_none()
}
fn validate(s: &Selector, kind: Kind) -> Result<(), AppError> {
    let valid = match (&s.id, &s.name) {
        (Some(id), None) => match kind {
            Kind::Image => image_id(id),
            Kind::Network => network_id(id),
            Kind::Volume => false,
        },
        (None, Some(v)) => match kind {
            Kind::Image => image_reference(v),
            _ => name(v),
        },
        _ => false,
    };
    if valid {
        Ok(())
    } else {
        Err(AppError::arguments(
            "docker_select",
            "Provide one exact full image sha256: digest, network ID, or exact object name; volumes support names only. Prefixes, nulls, wildcards and credential-bearing references are invalid",
        ))
    }
}
fn text(client: &CoreClient, v: &str, limit: usize) -> String {
    let safe = client.redact(v);
    if safe != v {
        return client.redaction_marker().into();
    }
    let mut end = v.len().min(limit);
    while !v.is_char_boundary(end) {
        end -= 1;
    }
    v[..end].to_owned()
}
fn date(client: &CoreClient, v: Option<String>) -> Option<String> {
    v.filter(|v| {
        v.len() <= 64
            && !v.is_empty()
            && v.bytes()
                .all(|b| b.is_ascii_digit() || b"TtZz-+:. ".contains(&b))
    })
    .map(|v| text(client, &v, 64))
}
fn token(v: Option<String>, allowed: &[&str]) -> Option<String> {
    v.map(|v| {
        if allowed.contains(&v.as_str()) {
            v
        } else {
            "other".into()
        }
    })
}
fn bytes(v: Option<i64>) -> Option<i64> {
    v.filter(|v| *v >= 0)
}
fn refs(client: &CoreClient, values: Vec<String>) -> (Vec<String>, bool) {
    let incomplete =
        values.len() > 4 || values.iter().any(|v| !image_reference(v) || v.len() > 256);
    (
        values
            .into_iter()
            .take(4)
            .filter(|v| image_reference(v) && v.len() <= 256)
            .map(|v| text(client, &v, 256))
            .collect(),
        incomplete,
    )
}
fn offsets(page: u64, size: u64) -> Result<usize, AppError> {
    if !(1..=100).contains(&size) || page.checked_mul(size).is_none_or(|n| n > SCAN as u64) {
        return Err(AppError::arguments(
            "docker_list",
            "Use page_size 1–100 and a local offset at most 512",
        ));
    }
    Ok((page * size) as usize)
}
fn page<T>(values: Vec<T>, page: u64, size: u64, observed: usize) -> Result<Page<T>, AppError> {
    let offset = offsets(page, size)?;
    let count = values.len();
    Ok(Page {
        items: values
            .into_iter()
            .skip(offset)
            .take(size as usize)
            .collect(),
        page,
        page_size: size,
        next_page: (offset + (size as usize) < count).then_some(page + 1),
        observed_count: observed,
        scan_incomplete: observed > SCAN,
    })
}
fn finish<T: Serialize>(
    client: &CoreClient,
    server: Server,
    data: T,
    fresh: bool,
) -> Result<Observation<T>, AppError> {
    let value = Observation {
        server,
        data,
        source: if fresh { "fresh_inspect" } else { "core_cache" },
        evidence_limit: if fresh { FRESH } else { CACHE },
    };
    if serde_json::to_vec(&inventory::projected_value(client, &value))
        .map_or(true, |v| v.len() > 96 * 1024)
    {
        Err(AppError::new(
            ErrorKind::ResponseTooLarge,
            "docker_observation",
            None,
        ))
    } else {
        Ok(value)
    }
}
async fn server(client: &CoreClient, target: DetailInput) -> Result<Server, AppError> {
    if target
        .name
        .as_ref()
        .is_some_and(|v| v.len() > 128 || v.chars().any(char::is_control))
    {
        return Err(AppError::arguments(
            "GetServer",
            "Use an exact bounded Server name or stable Komodo ID",
        ));
    }
    let (s, full) = inventory::resolve_server(client, target).await?;
    if full.template {
        return Err(AppError::new(
            ErrorKind::UnsupportedTarget,
            "docker_observation",
            None,
        ));
    }
    Ok(Server {
        id: s.id,
        name: text(client, &s.name, 128),
        cached_state: s.info.state.to_string().to_ascii_lowercase(),
        enabled: full.config.enabled,
        infrastructure_error_present: s.info.err.is_some(),
    })
}
fn select<T>(
    client: &CoreClient,
    items: Vec<T>,
    s: &Selector,
    identity: impl Fn(&T) -> (&str, Vec<&str>),
) -> Result<T, AppError> {
    if items.len() > SCAN {
        return Err(AppError::new(
            ErrorKind::IncompleteResolution,
            "docker_select",
            None,
        ));
    }
    let mut seen = BTreeSet::new();
    let mut matches = Vec::new();
    for item in items {
        let (id, names) = identity(&item);
        if id.is_empty() || !seen.insert(id.to_owned()) {
            return Err(invalid());
        }
        if s.id.as_deref() == Some(id) || s.name.as_deref().is_some_and(|n| names.contains(&n)) {
            matches.push(item);
        }
    }
    if matches.len() > 1 {
        return Err(AppError {
            candidates: Some(
                matches
                    .iter()
                    .take(5)
                    .map(|i| {
                        let (id, names) = identity(i);
                        Candidate {
                            id: text(client, id, 128),
                            name: names
                                .first()
                                .filter(|v| name(v) || image_reference(v))
                                .map(|v| text(client, v, 128))
                                .unwrap_or_else(|| "[omitted]".into()),
                        }
                    })
                    .collect(),
            ),
            ..AppError::new(ErrorKind::AmbiguousTarget, "docker_select", None)
        });
    }
    matches.pop().ok_or_else(|| AppError { message:"No exact object was present in the received cache. This does not prove absence on Docker; refresh upstream inventory and inspect its coverage. No broader target was selected.", ..AppError::new(ErrorKind::NotFound,"docker_select",None) })
}
fn image_summary(client: &CoreClient, v: ImageListItem) -> Result<ImageSummary, AppError> {
    if !image_id(&v.id) {
        return Err(invalid());
    }
    let (tags, t) = refs(client, v.tags);
    let (digests, d) = refs(client, v.digests);
    Ok(ImageSummary {
        id: v.id,
        tags,
        digests,
        references_incomplete: t || d,
        created_unix_seconds: v.created,
        size_bytes: bytes(Some(v.size)),
        cached_in_use: v.in_use,
    })
}
fn image_detail(client: &CoreClient, v: Image) -> Result<ImageDetail, AppError> {
    let id = v.id.filter(|v| image_id(v)).ok_or_else(invalid)?;
    let (tags, t) = refs(client, v.repo_tags.unwrap_or_default());
    let (digests, d) = refs(client, v.repo_digests.unwrap_or_default());
    Ok(ImageDetail {
        id,
        tags,
        digests,
        references_incomplete: t || d,
        created: date(client, v.created),
        architecture: token(
            v.architecture,
            &[
                "amd64", "arm64", "arm", "386", "ppc64le", "s390x", "riscv64",
            ],
        ),
        os: token(v.os, &["linux", "windows", "darwin", "freebsd"]),
        size_bytes: bytes(v.size),
        rootfs_layer_count: v.root_fs.map(|v| v.layers.len()),
    })
}
fn network_list(client: &CoreClient, v: NetworkListItem) -> Result<NetworkSummary, AppError> {
    let id = v.id.filter(|v| network_id(v)).ok_or_else(invalid)?;
    if v.name.as_ref().is_some_and(|v| !name(v)) {
        return Err(invalid());
    }
    Ok(NetworkSummary {
        id,
        name: v.name.map(|v| text(client, &v, 128)),
        driver: token(
            v.driver,
            &["bridge", "host", "null", "overlay", "macvlan", "ipvlan"],
        ),
        scope: token(v.scope, &["local", "global", "swarm"]),
        created: date(client, v.created),
        enable_ipv6: v.enable_ipv6,
        internal: v.internal,
        attachable: v.attachable,
        ingress: v.ingress,
        cached_in_use: Some(v.in_use),
        attached_container_count: None,
    })
}
fn network_detail(client: &CoreClient, v: Network) -> Result<NetworkSummary, AppError> {
    let count = v.containers.len();
    let mut out = network_list(
        client,
        NetworkListItem {
            id: v.id,
            name: v.name,
            created: v.created,
            scope: v.scope,
            driver: v.driver,
            enable_ipv6: v.enable_ipv6,
            internal: v.internal,
            attachable: v.attachable,
            ingress: v.ingress,
            ipam_driver: None,
            ipam_subnet: None,
            ipam_gateway: None,
            in_use: false,
        },
    )?;
    out.cached_in_use = None;
    out.attached_container_count = Some(count);
    Ok(out)
}
fn volume_list(client: &CoreClient, v: VolumeListItem) -> Result<VolumeSummary, AppError> {
    if !name(&v.name) {
        return Err(invalid());
    }
    Ok(VolumeSummary {
        name: text(client, &v.name, 128),
        driver: if v.driver == "local" {
            "local".into()
        } else {
            "other".into()
        },
        scope: serde_json::to_value(v.scope)
            .ok()
            .and_then(|v| v.as_str().map(str::to_owned))
            .unwrap_or_default(),
        created: date(client, v.created),
        size_bytes: bytes(v.size),
        cached_in_use: Some(v.in_use),
        reference_count: None,
    })
}
fn volume_detail(client: &CoreClient, v: Volume) -> Result<VolumeSummary, AppError> {
    let size = v.usage_data.as_ref().and_then(|v| bytes(Some(v.size)));
    let count = v.usage_data.and_then(|v| bytes(Some(v.ref_count)));
    let mut out = volume_list(
        client,
        VolumeListItem {
            name: v.name,
            driver: v.driver,
            mountpoint: String::new(),
            created: v.created_at,
            scope: v.scope,
            size,
            in_use: false,
        },
    )?;
    out.cached_in_use = None;
    out.reference_count = count;
    Ok(out)
}
async fn selected_image(
    client: &CoreClient,
    server: &str,
    s: &Selector,
) -> Result<Image, AppError> {
    let items = client
        .read(ListImages {
            server: server.into(),
        })
        .await?;
    if items.iter().any(|v| !image_id(&v.id)) {
        return Err(invalid());
    }
    let selected = select(client, items, s, |i| {
        (
            &i.id,
            std::iter::once(i.name.as_str())
                .chain(i.tags.iter().map(String::as_str))
                .chain(i.digests.iter().map(String::as_str))
                .collect(),
        )
    })?;
    let full = client
        .read(InspectImage {
            server: server.into(),
            image: selected.id.clone(),
        })
        .await?;
    if full.id.as_deref() != Some(&selected.id)
        || s.name.as_ref().is_some_and(|n| {
            n != &selected.id
                && !full.repo_tags.as_ref().is_some_and(|v| v.contains(n))
                && !full.repo_digests.as_ref().is_some_and(|v| v.contains(n))
        })
    {
        return Err(invalid());
    }
    Ok(full)
}
pub async fn images(
    client: &CoreClient,
    input: ListInput,
) -> Result<Observation<Page<ImageSummary>>, AppError> {
    offsets(input.page, input.page_size)?;
    let client = client.bounded_evidence();
    let server = server(&client, input.target).await?;
    let mut values = client
        .read(ListImages {
            server: server.id.clone(),
        })
        .await?;
    let observed = values.len();
    values.truncate(SCAN);
    values.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen = BTreeSet::new();
    let values = values
        .into_iter()
        .map(|v| {
            if !seen.insert(v.id.clone()) {
                return Err(invalid());
            }
            image_summary(&client, v)
        })
        .collect::<Result<Vec<_>, _>>()?;
    finish(
        &client,
        server,
        page(values, input.page, input.page_size, observed)?,
        false,
    )
}
pub async fn image(
    client: &CoreClient,
    input: ObjectInput,
) -> Result<Observation<ImageDetail>, AppError> {
    validate(&input.object, Kind::Image)?;
    let client = client.bounded_evidence();
    let server = server(&client, input.target).await?;
    let full = selected_image(&client, &server.id, &input.object).await?;
    finish(&client, server, image_detail(&client, full)?, true)
}
pub async fn history(
    client: &CoreClient,
    input: HistoryInput,
) -> Result<Observation<ImageHistory>, AppError> {
    validate(&input.object, Kind::Image)?;
    offsets(input.page, input.page_size)?;
    let client = client.bounded_evidence();
    let server = server(&client, input.target).await?;
    let full = selected_image(&client, &server.id, &input.object).await?;
    let id = full.id.ok_or_else(invalid)?;
    let values = client
        .read(ListImageHistory {
            server: server.id.clone(),
            image: id.clone(),
        })
        .await?;
    let observed = values.len();
    let layers = values
        .into_iter()
        .take(SCAN)
        .enumerate()
        .map(|(index, v)| {
            let id = if image_id(&v.id) {
                Some(v.id)
            } else if v.id == "<missing>" {
                None
            } else {
                return Err(invalid());
            };
            Ok(Layer {
                index,
                id,
                created_unix_seconds: v.created,
                size_bytes: bytes(Some(v.size)),
            })
        })
        .collect::<Result<Vec<_>, AppError>>()?;
    finish(
        &client,
        server,
        ImageHistory {
            image_id: id,
            layers: page(layers, input.page, input.page_size, observed)?,
        },
        true,
    )
}
pub async fn networks(
    client: &CoreClient,
    input: ListInput,
) -> Result<Observation<Page<NetworkSummary>>, AppError> {
    offsets(input.page, input.page_size)?;
    let client = client.bounded_evidence();
    let server = server(&client, input.target).await?;
    let mut values = client
        .read(ListNetworks {
            server: server.id.clone(),
        })
        .await?;
    let observed = values.len();
    values.truncate(SCAN);
    values.sort_by(|a, b| a.id.cmp(&b.id));
    let mut seen = BTreeSet::new();
    let values = values
        .into_iter()
        .map(|v| {
            if !seen.insert(v.id.clone()) {
                return Err(invalid());
            }
            network_list(&client, v)
        })
        .collect::<Result<Vec<_>, _>>()?;
    finish(
        &client,
        server,
        page(values, input.page, input.page_size, observed)?,
        false,
    )
}
pub async fn network(
    client: &CoreClient,
    input: ObjectInput,
) -> Result<Observation<NetworkSummary>, AppError> {
    validate(&input.object, Kind::Network)?;
    let client = client.bounded_evidence();
    let server = server(&client, input.target).await?;
    let values = client
        .read(ListNetworks {
            server: server.id.clone(),
        })
        .await?;
    if values
        .iter()
        .any(|v| !v.id.as_deref().is_some_and(network_id))
    {
        return Err(invalid());
    }
    let selected = select(&client, values, &input.object, |v| {
        (
            v.id.as_deref().unwrap_or_default(),
            v.name.as_deref().into_iter().collect(),
        )
    })?;
    let id = selected.id.ok_or_else(invalid)?;
    let full = client
        .read(InspectNetwork {
            server: server.id.clone(),
            network: id.clone(),
        })
        .await?;
    if full.id.as_deref() != Some(&id)
        || input
            .object
            .name
            .as_deref()
            .is_some_and(|n| full.name.as_deref() != Some(n))
    {
        return Err(invalid());
    }
    finish(&client, server, network_detail(&client, full)?, true)
}
pub async fn volumes(
    client: &CoreClient,
    input: ListInput,
) -> Result<Observation<Page<VolumeSummary>>, AppError> {
    offsets(input.page, input.page_size)?;
    let client = client.bounded_evidence();
    let server = server(&client, input.target).await?;
    let mut values = client
        .read(ListVolumes {
            server: server.id.clone(),
        })
        .await?;
    let observed = values.len();
    values.truncate(SCAN);
    values.sort_by(|a, b| a.name.cmp(&b.name));
    let mut seen = BTreeSet::new();
    let values = values
        .into_iter()
        .map(|v| {
            if !seen.insert(v.name.clone()) {
                return Err(invalid());
            }
            volume_list(&client, v)
        })
        .collect::<Result<Vec<_>, _>>()?;
    finish(
        &client,
        server,
        page(values, input.page, input.page_size, observed)?,
        false,
    )
}
pub async fn volume(
    client: &CoreClient,
    input: ObjectInput,
) -> Result<Observation<VolumeSummary>, AppError> {
    validate(&input.object, Kind::Volume)?;
    let client = client.bounded_evidence();
    let server = server(&client, input.target).await?;
    let values = client
        .read(ListVolumes {
            server: server.id.clone(),
        })
        .await?;
    if values.iter().any(|v| !name(&v.name)) {
        return Err(invalid());
    }
    let selected = select(&client, values, &input.object, |v| (&v.name, vec![&v.name]))?;
    let full = client
        .read(InspectVolume {
            server: server.id.clone(),
            volume: selected.name.clone(),
        })
        .await?;
    if full.name != selected.name {
        return Err(invalid());
    }
    finish(&client, server, volume_detail(&client, full)?, true)
}
