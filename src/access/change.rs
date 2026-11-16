//! Private authorization snapshots and one narrowly typed administration write.
use super::*;
use crate::configuration::Preview;
use komodo_client::{api::write::*, entities::permission::PermissionLevelAndSpecifics};
use serde::Serialize;
use serde_json::{Value, json};
use std::{collections::BTreeSet, time::Instant};

struct Prepared {
    op: &'static str,
    params: Value,
    baseline: Value,
    expected: Option<Value>,
    entity_kind: &'static str,
    entity_id: Option<String>,
    entity_name: String,
    result: AccessResult,
}
fn policy(client: &CoreClient) -> Result<(), AppError> {
    common::require_edit(client)?;
    crate::operations::require_operate(client, OP)
}
fn param<T: Serialize>(v: T) -> Value {
    serde_json::to_value(v).expect("typed administration request")
}
fn entity(client: &CoreClient, kind: &str, raw: &Value) -> Result<EntityView, AppError> {
    if kind == "user" {
        Ok(EntityView::User(user_view(client, raw)?))
    } else {
        Ok(EntityView::Group(group_view(client, raw)?))
    }
}
fn result(
    client: &CoreClient,
    action: &'static str,
    target: (&'static str, Option<&Value>, &str),
    after: Option<EntityView>,
    broad: bool,
    super_admin: bool,
) -> Result<AccessResult, AppError> {
    let (kind, raw, entity_name) = target;
    Ok(AccessResult {
        action,
        outcome: "preview",
        resource: ConfigurationTarget {
            kind,
            id: raw.map(id).transpose()?.map(String::from),
            name: label(client, entity_name),
        },
        before: raw.map(|v| entity(client, kind, v)).transpose()?,
        after,
        broad_scope: broad,
        requires_super_admin: super_admin,
        preview_id: None,
        writes_submitted: 0,
        acknowledgement_valid: false,
        saved_state_matches: false,
        submission_error: None,
        observation_error: None,
        cleanup: None,
        effects: EFFECTS.to_vec(),
    })
}
fn new_user(client: &CoreClient, username: &str) -> EntityView {
    EntityView::User(UserView {
        id: None,
        username: label(client, username),
        service_user: true,
        enabled: true,
        admin: false,
        super_admin: false,
        create_servers: false,
        create_builds: false,
        direct_type_grants: vec![],
        grants_complete: true,
        authentication_details: "omitted",
        service_description: "omitted; use original private input for description edits",
    })
}
fn new_group(client: &CoreClient, n: &str) -> EntityView {
    EntityView::Group(GroupView {
        id: None,
        name: label(client, n),
        everyone: false,
        member_ids: vec![],
        member_count: 0,
        members_complete: true,
        direct_type_grants: vec![],
        grants_complete: true,
        visibility_limit: GROUP_VISIBILITY,
    })
}
fn text(s: &str, max: usize) -> Result<(), AppError> {
    if s.len() > max || s.contains('\0') {
        return Err(args(
            "Use bounded original text without NUL or display redaction markers",
        ));
    }
    Ok(())
}
fn namespace(a: &[Value], key: &str) -> Value {
    json!(
        a.iter()
            .map(|v| json!({"id":v["_id"]["$oid"],"name":v[key]}))
            .collect::<Vec<_>>()
    )
}
fn unprotected(actor: &Value, u: &Value, remove: bool) -> Result<(), AppError> {
    if u["super_admin"] == true
        || (u["admin"] == true && actor["super_admin"] != true)
        || (remove && (u["_id"]["$oid"] == actor["id"] || u["username"] == actor["username"]))
    {
        return Err(denied());
    }
    Ok(())
}
fn members(g: &Value) -> Result<Vec<String>, AppError> {
    let a = g["users"].as_array().ok_or_else(invalid)?;
    if a.len() > 512 {
        return Err(AppError::new(ErrorKind::ResponseTooLarge, OP, None));
    }
    let mut seen = BTreeSet::new();
    a.iter()
        .map(|v| {
            let s = v
                .as_str()
                .filter(|s| crate::deletion::object_id(s))
                .ok_or_else(invalid)?;
            if !seen.insert(s) {
                return Err(invalid());
            }
            Ok(s.into())
        })
        .collect()
}
fn merge_grant(current: &Value, p: &GrantPatch) -> Result<GrantView, AppError> {
    if p.add_specific.len() > 5 || p.remove_specific.len() > 5 {
        return Err(args(
            "Use at most five unique specific flags per add/remove list",
        ));
    }
    let mut g = grant(current, true)?;
    let add = p.add_specific.iter().copied().collect::<BTreeSet<_>>();
    let remove = p.remove_specific.iter().copied().collect::<BTreeSet<_>>();
    if add.len() != p.add_specific.len()
        || remove.len() != p.remove_specific.len()
        || !add.is_disjoint(&remove)
    {
        return Err(args(
            "Specific additions/removals must be unique and disjoint; omitted flags are preserved",
        ));
    }
    if let Some(level) = p.level {
        g.level = level;
    }
    let mut flags = g.specific.into_iter().collect::<BTreeSet<_>>();
    flags.retain(|f| !remove.contains(f));
    flags.extend(add);
    g.specific = flags.into_iter().collect();
    Ok(g)
}
fn same_grant(a: &Value, b: &Value) -> bool {
    match (grant(a, true), grant(b, true)) {
        (Ok(a), Ok(b)) => a.level == b.level && a.specific == b.specific,
        _ => false,
    }
}
fn comparable(mut v: Value) -> Value {
    if let Some(o) = v.as_object_mut() {
        o.remove("updated_at");
    }
    v
}

async fn prepare(client: &CoreClient, change: &Change) -> Result<Prepared, AppError> {
    common::validate_payload(client, change)?;
    let caller = actor(client).await?;
    let mut baseline = json!({"caller":caller});
    let (op, params, expected, kind, entity_id, entity_name, r) = match change {
        Change::CreateServiceUser {
            username,
            description,
        } => {
            if !name(username) {
                return Err(args(
                    "Use an exact nonempty service username of at most 128 bytes without controls; Core validates its username syntax",
                ));
            }
            text(description, 4096)?;
            let (a, _) = users(client, true, UserKind::All).await?;
            if a.iter().any(|u| u["username"] == *username) {
                return Err(stale());
            }
            baseline["namespace"] = namespace(&a, "username");
            let expected = json!({"username":username,"enabled":true,"admin":false,"super_admin":false,"create_server_permissions":false,"create_build_permissions":false,"all":{},"config":{"type":"Service","data":{"description":description}}});
            let r = result(
                client,
                "create_service_user",
                ("user", None, username),
                Some(new_user(client, username)),
                false,
                false,
            )?;
            (
                "CreateServiceUser",
                param(CreateServiceUser {
                    username: username.clone(),
                    description: description.clone(),
                }),
                Some(expected),
                "user",
                None,
                username.clone(),
                r,
            )
        }
        Change::CreateGroup { name: n } => {
            if !name(n) {
                return Err(args(
                    "Use an exact nonempty group name of at most 128 bytes without controls; names are not normalized",
                ));
            }
            let (a, _) = groups(client, true).await?;
            if a.iter().any(|g| g["name"] == *n) {
                return Err(stale());
            }
            baseline["namespace"] = namespace(&a, "name");
            let expected = json!({"name":n,"everyone":false,"users":[],"all":{}});
            let r = result(
                client,
                "create_group",
                ("group", None, n),
                Some(new_group(client, n)),
                false,
                false,
            )?;
            (
                "CreateUserGroup",
                param(CreateUserGroup { name: n.clone() }),
                Some(expected),
                "group",
                None,
                n.clone(),
                r,
            )
        }
        Change::SetServiceUserDescription {
            target,
            description,
        } => {
            text(description, 4096)?;
            let u = user(client, target).await?;
            if u["config"]["type"] != "Service" || !u["config"]["data"]["description"].is_string() {
                return Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None));
            }
            let uid = id(&u)?.to_owned();
            let username = u["username"].as_str().expect("checked").to_owned();
            let mut after = u.clone();
            after["config"]["data"]["description"] = json!(description);
            baseline["entity"] = u.clone();
            let r = result(
                client,
                "set_service_user_description",
                ("user", Some(&u), &username),
                Some(entity(client, "user", &after)?),
                false,
                false,
            )?;
            (
                "UpdateServiceUserDescription",
                param(UpdateServiceUserDescription {
                    username: username.clone(),
                    description: description.clone(),
                }),
                Some(after),
                "user",
                Some(uid),
                username,
                r,
            )
        }
        Change::SetUserBase {
            target,
            enabled,
            create_servers,
            create_builds,
        } => {
            let u = user(client, target).await?;
            unprotected(&caller, &u, *enabled == Some(false))?;
            let uid = id(&u)?.to_owned();
            let username = u["username"].as_str().expect("checked").to_owned();
            if u["_id"]["$oid"] == caller["id"] && *enabled == Some(false) {
                return Err(denied());
            }
            let mut after = u.clone();
            for (k, v) in [
                ("enabled", enabled),
                ("create_server_permissions", create_servers),
                ("create_build_permissions", create_builds),
            ] {
                if let Some(b) = v {
                    after[k] = json!(b);
                }
            }
            baseline["entity"] = u.clone();
            let r = result(
                client,
                "set_user_base",
                ("user", Some(&u), &username),
                Some(entity(client, "user", &after)?),
                false,
                u["admin"] == true,
            )?;
            (
                "UpdateUserBasePermissions",
                param(UpdateUserBasePermissions {
                    user_id: uid.clone(),
                    enabled: *enabled,
                    create_servers: *create_servers,
                    create_builds: *create_builds,
                }),
                Some(after),
                "user",
                Some(uid),
                username,
                r,
            )
        }
        Change::SetUserAdmin { target, admin } => {
            if caller["super_admin"] != true {
                return Err(denied());
            }
            let u = user(client, target).await?;
            if u["super_admin"] == true
                || u["enabled"] != true
                || (u["_id"]["$oid"] == caller["id"] && !admin)
            {
                return Err(denied());
            }
            let uid = id(&u)?.to_owned();
            let username = u["username"].as_str().expect("checked").to_owned();
            let mut after = u.clone();
            after["admin"] = json!(admin);
            baseline["entity"] = u.clone();
            let r = result(
                client,
                "set_user_admin",
                ("user", Some(&u), &username),
                Some(entity(client, "user", &after)?),
                true,
                true,
            )?;
            (
                "UpdateUserAdmin",
                param(UpdateUserAdmin {
                    user_id: uid.clone(),
                    admin: *admin,
                }),
                Some(after),
                "user",
                Some(uid),
                username,
                r,
            )
        }
        Change::DeleteUser { target } => {
            let u = user(client, target).await?;
            unprotected(&caller, &u, true)?;
            let uid = id(&u)?.to_owned();
            let username = u["username"].as_str().expect("checked").to_owned();
            baseline["entity"] = u.clone();
            let (g, _) = groups(client, true).await?;
            baseline["groups"] = json!(g);
            let s = SubjectView {
                kind: "user",
                id: uid.clone(),
                name: label(client, &username),
            };
            let (p, _) = permissions(client, &s, true).await?;
            baseline["permissions"] = json!(p);
            let r = result(
                client,
                "delete_user",
                ("user", Some(&u), &username),
                None,
                true,
                u["admin"] == true,
            )?;
            (
                "DeleteUser",
                param(DeleteUser { user: uid.clone() }),
                None,
                "user",
                Some(uid),
                username,
                r,
            )
        }
        Change::RenameGroup { target, name: n } => {
            if !name(n) {
                return Err(args(
                    "Use an exact nonempty bounded group name without controls",
                ));
            }
            let g = group(client, target).await?;
            let gid = id(&g)?.to_owned();
            let old = g["name"].as_str().expect("checked").to_owned();
            let (a, _) = groups(client, true).await?;
            if a.iter().any(|v| v["name"] == *n && v["_id"]["$oid"] != gid) {
                return Err(stale());
            }
            baseline["namespace"] = namespace(&a, "name");
            baseline["entity"] = g.clone();
            let mut after = g.clone();
            after["name"] = json!(n);
            let r = result(
                client,
                "rename_group",
                ("group", Some(&g), &old),
                Some(entity(client, "group", &after)?),
                g["everyone"] == true,
                false,
            )?;
            (
                "RenameUserGroup",
                param(RenameUserGroup {
                    id: gid.clone(),
                    name: n.clone(),
                }),
                Some(after),
                "group",
                Some(gid),
                old,
                r,
            )
        }
        Change::DeleteGroup { target } => {
            let g = group(client, target).await?;
            let gid = id(&g)?.to_owned();
            let n = g["name"].as_str().expect("checked").to_owned();
            baseline["entity"] = g.clone();
            let s = SubjectView {
                kind: "group",
                id: gid.clone(),
                name: label(client, &n),
            };
            let (p, _) = permissions(client, &s, true).await?;
            baseline["permissions"] = json!(p);
            let r = result(
                client,
                "delete_group",
                ("group", Some(&g), &n),
                None,
                true,
                false,
            )?;
            (
                "DeleteUserGroup",
                param(DeleteUserGroup { id: gid.clone() }),
                None,
                "group",
                Some(gid),
                n,
                r,
            )
        }
        Change::SetGroupMember {
            group: target,
            user: ut,
            present,
        } => {
            let g = group(client, target).await?;
            let u = user(client, ut).await?;
            let gid = id(&g)?.to_owned();
            let uid = id(&u)?.to_owned();
            let n = g["name"].as_str().expect("checked").to_owned();
            let mut ids = members(&g)?;
            if *present && !ids.contains(&uid) {
                ids.push(uid.clone());
            }
            if !present {
                ids.retain(|id| id != &uid);
            }
            let mut after = g.clone();
            after["users"] = json!(ids);
            baseline["entity"] = g.clone();
            baseline["member"] = u;
            let r = result(
                client,
                "set_group_member",
                ("group", Some(&g), &n),
                Some(entity(client, "group", &after)?),
                g["everyone"] == true,
                false,
            )?;
            let (op, params) = if *present {
                (
                    "AddUserToUserGroup",
                    param(AddUserToUserGroup {
                        user_group: gid.clone(),
                        user: uid,
                    }),
                )
            } else {
                (
                    "RemoveUserFromUserGroup",
                    param(RemoveUserFromUserGroup {
                        user_group: gid.clone(),
                        user: uid,
                    }),
                )
            };
            (op, params, Some(after), "group", Some(gid), n, r)
        }
        Change::SetGroupEveryone { target, everyone } => {
            let g = group(client, target).await?;
            let gid = id(&g)?.to_owned();
            let n = g["name"].as_str().expect("checked").to_owned();
            let mut after = g.clone();
            after["everyone"] = json!(everyone);
            baseline["entity"] = g.clone();
            let r = result(
                client,
                "set_group_everyone",
                ("group", Some(&g), &n),
                Some(entity(client, "group", &after)?),
                true,
                false,
            )?;
            (
                "SetEveryoneUserGroup",
                param(SetEveryoneUserGroup {
                    user_group: gid.clone(),
                    everyone: *everyone,
                }),
                Some(after),
                "group",
                Some(gid),
                n,
                r,
            )
        }
        Change::ChangePermission {
            subject: selection,
            scope,
            patch,
        } => {
            let (s, v) = subject(client, selection).await?;
            if s.kind == "user" && (v["admin"] == true || v["enabled"] != true) {
                return Err(denied());
            }
            baseline["entity"] = v.clone();
            let (sv, current, exists, op, params, expected) = match scope {
                PermissionScope::Resource {
                    resource_kind,
                    target,
                } => {
                    let (sv, raw) = resource(client, *resource_kind, target).await?;
                    baseline["resource"] = raw;
                    let ScopeView::Resource { id: rid, .. } = &sv else {
                        unreachable!()
                    };
                    let target_wire = json!({"type":core_kind(*resource_kind),"id":rid});
                    let (a, _) = permissions(client, &s, true).await?;
                    let records = a
                        .iter()
                        .filter(|v| v["resource_target"] == target_wire)
                        .collect::<Vec<_>>();
                    if records.len() > 1 {
                        return Err(invalid());
                    }
                    let current = records.first().map_or(
                        json!("None"),
                        |v| json!({"level":v["level"],"specific":v["specific"]}),
                    );
                    let g = merge_grant(&current, patch)?;
                    let p: PermissionLevelAndSpecifics =
                        serde_json::from_value(wire_grant(&g)).map_err(|_| invalid())?;
                    baseline["permissions"] = json!(a);
                    let params = param(UpdatePermissionOnTarget {
                        user_target: wire_subject(&s),
                        resource_target: serde_json::from_value(target_wire.clone())
                            .expect("target"),
                        permission: p,
                    });
                    let expected = json!({"target":target_wire,"grant":wire_grant(&g),"record_exists":!records.is_empty() || !same_grant(&current,&wire_grant(&g))});
                    (
                        sv,
                        current,
                        !records.is_empty(),
                        "UpdatePermissionOnTarget",
                        params,
                        expected,
                    )
                }
                PermissionScope::ResourceType { resource_kind } => {
                    let current = v["all"]
                        .get(core_kind(*resource_kind))
                        .cloned()
                        .unwrap_or(json!("None"));
                    let exists = v["all"].get(core_kind(*resource_kind)).is_some();
                    let g = merge_grant(&current, patch)?;
                    let p: PermissionLevelAndSpecifics =
                        serde_json::from_value(wire_grant(&g)).map_err(|_| invalid())?;
                    let params = param(UpdatePermissionOnResourceType {
                        user_target: wire_subject(&s),
                        resource_type: serde_json::from_value(json!(core_kind(*resource_kind)))
                            .expect("kind"),
                        permission: p,
                    });
                    let mut after = v.clone();
                    after["all"][core_kind(*resource_kind)] = wire_grant(&g);
                    (
                        ScopeView::ResourceType {
                            resource_kind: *resource_kind,
                        },
                        current,
                        exists,
                        "UpdatePermissionOnResourceType",
                        params,
                        after,
                    )
                }
            };
            let before = grant(&current, true)?;
            let after = merge_grant(&current, patch)?;
            let no_op = before.level == after.level && before.specific == after.specific;
            let broad = matches!(sv, ScopeView::ResourceType { .. })
                || (s.kind == "group" && v["everyone"] == true);
            let mut r = result(
                client,
                "change_permission",
                (s.kind, None, &s.name),
                None,
                broad,
                false,
            )?;
            r.resource.id = Some(s.id.clone());
            r.before = Some(EntityView::Permission {
                subject: s.clone(),
                scope: sv.clone(),
                grant: before,
                direct_record_exists: exists,
            });
            r.after = Some(EntityView::Permission {
                subject: s.clone(),
                scope: sv,
                grant: after,
                direct_record_exists: exists || !no_op,
            });
            baseline["permission_noop"] = json!(no_op);
            (op, params, Some(expected), s.kind, Some(s.id), s.name, r)
        }
    };
    private_bound(&baseline)?;
    Ok(Prepared {
        op,
        params,
        baseline,
        expected,
        entity_kind: kind,
        entity_id,
        entity_name,
        result: r,
    })
}
impl Prepared {
    fn proof(&self, change: &Change) -> Value {
        json!({"change":change,"operation":self.op,"parameters":self.params,"baseline":self.baseline})
    }
    fn no_op(&self) -> bool {
        if self.result.action == "change_permission" {
            return self.baseline["permission_noop"] == true;
        }
        self.expected.as_ref().is_some_and(|e| {
            self.baseline
                .get("entity")
                .is_some_and(|v| comparable(v.clone()) == comparable(e.clone()))
        })
    }
}
pub async fn preview(client: &CoreClient, input: PreviewInput) -> Result<AccessResult, AppError> {
    common::require_edit(client)?;
    let p = prepare(&client.bounded_evidence(), &input.change).await?;
    let mut r = p.result.clone();
    let token = client.previews().insert_entry(Preview {
        created: Instant::now(),
        input: common::bound_input(OP, &input),
        baseline: p.baseline.clone(),
        prepared: p.proof(&input.change),
        result: p.result,
    })?;
    r.preview_id = Some(token);
    Ok(r)
}
pub(crate) async fn guard_write(
    client: &CoreClient,
    op: &'static str,
    params: &Value,
    proof: &Value,
) -> Result<(), AppError> {
    policy(client)?;
    let c: Change = serde_json::from_value(proof["change"].clone())
        .map_err(|_| AppError::new(ErrorKind::InvalidPreview, OP, None))?;
    if proof["operation"] != op || proof["parameters"] != *params {
        return Err(AppError::new(ErrorKind::InvalidPreview, OP, None));
    }
    let p = prepare(&client.bounded_evidence(), &c).await?;
    if p.op != op || p.params != *params || p.proof(&c) != *proof {
        return Err(stale());
    }
    Ok(())
}
async fn submit(
    client: &CoreClient,
    p: &Prepared,
    proof: &Value,
) -> Result<Result<Value, AppError>, AppError> {
    macro_rules! send {
        ($ty:ident) => {{
            let request: $ty = serde_json::from_value(p.params.clone()).map_err(|_| invalid())?;
            client.access_write_checked(request, proof).await
        }};
    }
    match p.op {
        "CreateServiceUser" => send!(CreateServiceUser),
        "UpdateServiceUserDescription" => send!(UpdateServiceUserDescription),
        "UpdateUserBasePermissions" => send!(UpdateUserBasePermissions),
        "UpdateUserAdmin" => send!(UpdateUserAdmin),
        "DeleteUser" => send!(DeleteUser),
        "CreateUserGroup" => send!(CreateUserGroup),
        "RenameUserGroup" => send!(RenameUserGroup),
        "DeleteUserGroup" => send!(DeleteUserGroup),
        "AddUserToUserGroup" => send!(AddUserToUserGroup),
        "RemoveUserFromUserGroup" => send!(RemoveUserFromUserGroup),
        "SetEveryoneUserGroup" => send!(SetEveryoneUserGroup),
        "UpdatePermissionOnTarget" => send!(UpdatePermissionOnTarget),
        "UpdatePermissionOnResourceType" => send!(UpdatePermissionOnResourceType),
        _ => Err(AppError::new(ErrorKind::UnsupportedTarget, OP, None)),
    }
}
fn subset(actual: &Value, expected: &Value) -> bool {
    expected
        .as_object()
        .is_some_and(|o| o.iter().all(|(k, v)| actual.get(k) == Some(v)))
}
fn valid_ack(p: &Prepared, ack: &Value) -> bool {
    if matches!(
        p.op,
        "UpdatePermissionOnTarget"
            | "UpdatePermissionOnResourceType"
            | "UpdateUserBasePermissions"
            | "UpdateUserAdmin"
    ) {
        return ack.as_object().is_some_and(|m| m.is_empty());
    }
    if id(ack).is_err()
        || p.entity_id
            .as_deref()
            .is_some_and(|i| id(ack).ok() != Some(i))
    {
        return false;
    }
    let key = if p.entity_kind == "user" {
        "username"
    } else {
        "name"
    };
    if p.expected.is_none() {
        return ack[key] == p.entity_name;
    }
    let expected = p.expected.as_ref().expect("some");
    if p.entity_id.is_none() {
        subset(ack, expected)
    } else {
        comparable(ack.clone()) == comparable(expected.clone())
    }
}
async fn observed_entity(
    client: &CoreClient,
    p: &Prepared,
    ack_id: Option<&str>,
) -> Result<Option<Value>, AppError> {
    actor(client).await?;
    let (a, _) = if p.entity_kind == "user" {
        users(client, true, UserKind::All).await?
    } else {
        groups(client, true).await?
    };
    // Group visibility becomes filtered if the same caller loses admin.
    // Recheck even when the directory has no matching row.
    actor(client).await?;
    let key = if p.entity_kind == "user" {
        "username"
    } else {
        "name"
    };
    let oid = p.entity_id.as_deref().or(ack_id);
    let selected = a
        .iter()
        .filter(|v| oid.map_or(v[key] == p.entity_name, |i| id(v).ok() == Some(i)))
        .collect::<Vec<_>>();
    if selected.len() > 1 {
        return Err(invalid());
    }
    let Some(v) = selected.first() else {
        return Ok(None);
    };
    let target = DetailInput {
        id: Some(id(v)?.into()),
        name: None,
    };
    let raw = if p.entity_kind == "user" {
        user(client, &target).await?
    } else {
        group(client, &target).await?
    };
    actor(client).await?;
    Ok(Some(raw))
}
async fn observe(
    client: &CoreClient,
    p: &Prepared,
    r: &mut AccessResult,
    ack_id: Option<&str>,
) -> Result<(), AppError> {
    if p.result.action == "change_permission" && p.op == "UpdatePermissionOnTarget" {
        actor(client).await?;
        let s = SubjectView {
            kind: p.entity_kind,
            id: p.entity_id.clone().expect("subject"),
            name: p.entity_name.clone(),
        };
        let (a, _) = permissions(client, &s, true).await?;
        let e = p.expected.as_ref().expect("expected");
        let record = a.iter().find(|v| v["resource_target"] == e["target"]);
        let actual = record.map_or(
            json!("None"),
            |v| json!({"level":v["level"],"specific":v["specific"]}),
        );
        r.saved_state_matches =
            same_grant(&actual, &e["grant"]) && record.is_some() == (e["record_exists"] == true);
        // Core updates only this record; inspect the rest for unexpected collateral changes.
        let before = p.baseline["permissions"].as_array().expect("stored");
        let untouched_before = before
            .iter()
            .filter(|v| v["resource_target"] != e["target"])
            .collect::<Vec<_>>();
        let untouched_after = a
            .iter()
            .filter(|v| v["resource_target"] != e["target"])
            .collect::<Vec<_>>();
        r.saved_state_matches &= untouched_before == untouched_after;
        let (_, parent) = subject(
            client,
            &if p.entity_kind == "user" {
                Subject::User {
                    target: DetailInput {
                        id: p.entity_id.clone(),
                        name: None,
                    },
                }
            } else {
                Subject::Group {
                    target: DetailInput {
                        id: p.entity_id.clone(),
                        name: None,
                    },
                }
            },
        )
        .await?;
        r.saved_state_matches &= comparable(parent) == comparable(p.baseline["entity"].clone());
    } else {
        let observed = observed_entity(client, p, ack_id).await?;
        r.saved_state_matches = match (&p.expected, &observed) {
            (None, None) => true,
            (Some(e), Some(v)) if p.entity_id.is_none() => subset(v, e),
            (Some(e), Some(v)) => comparable(v.clone()) == comparable(e.clone()),
            _ => false,
        };
        if let Some(v) = observed
            && p.result.action != "change_permission"
        {
            r.resource.id = Some(id(&v)?.into());
            r.after = Some(entity(client, p.entity_kind, &v)?);
        }
    }
    if !r.saved_state_matches {
        r.observation_error = Some(invalid());
    }
    Ok(())
}
async fn cleanup(client: &CoreClient, p: &Prepared) -> CleanupReport {
    let mut report = CleanupReport {
        visible_group_references: vec![],
        group_reference_count: None,
        direct_permission_records_remaining: None,
        observations_complete: true,
        credential_cleanup: "not_performed_or_verified; Core DeleteUser leaves API-key records, and cached authorization may outlive a write",
        errors: vec![],
        evidence_limit: "Complete bounded visible observations under the same current administrator; permission/visibility can race. No transaction, extra cleanup write, key erasure or other-user effective-access claim.",
    };
    if let Err(e) = actor(client).await {
        report.observations_complete = false;
        report.errors.push(e);
        return report;
    }
    let sid = p.entity_id.clone().expect("delete existing");
    if p.entity_kind == "user" {
        match groups(client, true).await {
            Ok((a, _)) => {
                if a.iter().any(|g| {
                    g["users"].as_array().is_none_or(|v| {
                        v.iter()
                            .any(|id| id.as_str().is_none_or(|s| !crate::deletion::object_id(s)))
                    })
                }) {
                    report.observations_complete = false;
                    report.errors.push(invalid());
                }
                let refs = a
                    .iter()
                    .filter(|g| {
                        g["users"]
                            .as_array()
                            .is_some_and(|a| a.iter().any(|v| v == &sid))
                    })
                    .collect::<Vec<_>>();
                report.group_reference_count = Some(refs.len());
                report.visible_group_references = refs
                    .into_iter()
                    .take(16)
                    .map(|g| SubjectView {
                        kind: "group",
                        id: id(g).expect("validated").into(),
                        name: label(client, g["name"].as_str().expect("checked")),
                    })
                    .collect();
            }
            Err(e) => {
                report.observations_complete = false;
                report.errors.push(e);
            }
        }
    }
    let s = SubjectView {
        kind: p.entity_kind,
        id: sid,
        name: p.entity_name.clone(),
    };
    match permissions(client, &s, true).await {
        Ok((a, _)) => report.direct_permission_records_remaining = Some(a.len()),
        Err(e) => {
            report.observations_complete = false;
            report.errors.push(e);
        }
    }
    if let Err(e) = actor(client).await {
        report.observations_complete = false;
        report.errors.push(e);
    }
    report
}
pub async fn apply(client: &CoreClient, input: ApplyInput) -> Result<AccessResult, AppError> {
    policy(client)?;
    let bound = PreviewInput {
        change: input.change.clone(),
    };
    let e = client
        .previews()
        .take_for::<AccessResult>(&input.preview_id, &common::bound_input(OP, &bound))?;
    let b = client.bounded_evidence();
    let p = prepare(&b, &input.change).await?;
    if p.baseline != e.baseline || p.proof(&input.change) != e.prepared {
        return Err(stale());
    }
    let mut r = e.result;
    r.preview_id = None;
    if p.no_op() {
        r.outcome = "no_op";
        r.saved_state_matches = true;
        return Ok(r);
    }
    let response = submit(client, &p, &e.prepared).await?;
    r.writes_submitted = 1;
    let mut ack_id = None;
    match response {
        Ok(v) if valid_ack(&p, &v) => {
            r.acknowledgement_valid = true;
            r.outcome = if p.expected.is_none() {
                "deleted"
            } else {
                "saved"
            };
            ack_id = id(&v).ok().map(String::from);
        }
        Ok(_) => {
            r.outcome = "submission_unknown";
            r.submission_error = Some(invalid());
        }
        Err(error) => {
            r.outcome = if error
                .status
                .is_some_and(|s| matches!(s, 400 | 401 | 403 | 404 | 405 | 409 | 422))
            {
                "rejected"
            } else {
                "submission_unknown"
            };
            r.submission_error = Some(error);
        }
    }
    if let Err(e) = observe(&b, &p, &mut r, ack_id.as_deref()).await {
        // A matching grant is insufficient if a later parent/authority read
        // fails. Never retain a partially established verification result.
        r.saved_state_matches = false;
        r.observation_error = Some(e);
    }
    if p.expected.is_none() {
        r.cleanup = Some(cleanup(&b, &p).await);
    }
    Ok(r)
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
    async fn shared_access_transport_denies_missing_grants_before_any_network() {
        for operate in [false, true] {
            for edit in [false, true] {
                let c = client(operate, edit);
                if !operate || !edit {
                    assert_eq!(
                        c.access_write_checked(
                            UpdateUserAdmin {
                                user_id: "000000000000000000000081".into(),
                                admin: true
                            },
                            &Value::Null
                        )
                        .await
                        .unwrap_err()
                        .kind,
                        ErrorKind::CapabilityDisabled
                    );
                } else {
                    assert_eq!(
                        c.access_write_checked(
                            UpdateUserAdmin {
                                user_id: "000000000000000000000081".into(),
                                admin: true
                            },
                            &Value::Null
                        )
                        .await
                        .unwrap_err()
                        .kind,
                        ErrorKind::InvalidPreview
                    );
                }
                assert_eq!(
                    c.access_write_checked(
                        CreateLocalUser {
                            username: "no-human".into(),
                            password: "never-submitted".into()
                        },
                        &Value::Null
                    )
                    .await
                    .unwrap_err()
                    .kind,
                    ErrorKind::UnsupportedTarget
                );
                assert_eq!(
                    c.access_write_checked(
                        SetUsersInUserGroup {
                            user_group: "000000000000000000000091".into(),
                            users: vec![]
                        },
                        &Value::Null
                    )
                    .await
                    .unwrap_err()
                    .kind,
                    ErrorKind::UnsupportedTarget
                );
                assert_eq!(
                    c.write_checked(CreateServiceUser {
                        username: "no-escape".into(),
                        description: String::new()
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
    fn legacy_permission_strings_are_explicit_and_missing_or_unknown_data_is_not_none() {
        assert!(grant(&json!("Read"), true).is_ok());
        assert!(grant(&json!({"level":"Read","specific":["Logs"]}), true).is_ok());
        for bad in [
            Value::Null,
            json!({}),
            json!({"specific":[]}),
            json!({"level":"Read"}),
            json!({"level":"Read","specific":["FutureFlag"]}),
            json!({"level":"Read","specific":[],"unknown":true}),
        ] {
            assert!(grant(&bad, true).is_err());
        }
    }
}
