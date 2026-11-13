//! Cleanup-update candidates; Core returns no update ID for definition deletes.
use std::collections::BTreeSet;

use komodo_client::{
    api::read::{GetUpdate, ListUpdates},
    entities::{update::Update, update::UpdateListItem},
};
use serde_json::{Value, json};

use super::{CleanupUpdate, ResourceKind, Target, invalid, object_id};
use crate::{
    client::CoreClient,
    error::{AppError, ErrorKind},
};

pub(super) struct History {
    pub ids: BTreeSet<String>,
    pub snapshot: Value,
    pub incomplete: bool,
    pub error: Option<AppError>,
}
fn target_matches(value: &impl serde::Serialize, t: &Target) -> bool {
    serde_json::to_value(value).is_ok_and(|v| {
        v["type"] == t.kind.core_name()
            && v["id"]
                .as_str()
                .is_some_and(|id| id.eq_ignore_ascii_case(&t.id))
    })
}
async fn list(client: &CoreClient, t: &Target) -> Result<(Vec<UpdateListItem>, bool), AppError> {
    let query=serde_json::from_value(json!({"target.type":t.kind.core_name(),"target.id":t.id,"operation":t.kind.delete_operation()})).map_err(|_|invalid())?;
    let page = client
        .read(ListUpdates {
            query: Some(query),
            page: 0,
        })
        .await?;
    if page.updates.len() > 100
        || page.next_page.is_some_and(|p| p != 1)
        || page.updates.iter().any(|u| {
            !object_id(&u.id)
                || !target_matches(&u.target, t)
                || u.operation.to_string() != t.kind.delete_operation()
        })
    {
        return Err(invalid());
    }
    let mut seen = BTreeSet::new();
    if page.updates.iter().any(|u| !seen.insert(u.id.clone())) {
        return Err(invalid());
    }
    Ok((page.updates, page.next_page.is_some()))
}
pub(super) async fn before(client: &CoreClient, t: &Target) -> Result<History, AppError> {
    match list(client, t).await {
        Ok((items, incomplete)) => {
            let ids = items
                .into_iter()
                .map(|u| u.id.to_ascii_lowercase())
                .collect::<BTreeSet<_>>();
            Ok(History {
                snapshot: json!({"ids":ids,"incomplete":incomplete}),
                ids,
                incomplete,
                error: None,
            })
        }
        Err(e) if e.kind == ErrorKind::InvalidResponse => Err(e),
        Err(e) => Ok(History {
            ids: BTreeSet::new(),
            snapshot: json!({"error":{"kind":e.kind,"operation":e.operation,"status":e.status}}),
            incomplete: true,
            error: Some(e),
        }),
    }
}
fn classify(u: &Update, kind: ResourceKind) -> (&'static str, Vec<String>) {
    let mut stages = vec![];
    let mut failed = false;
    let mut skipped = false;
    let mut command = false;
    for log in &u.logs {
        // Deleted Toml/stdout can contain the entire secret-bearing definition.
        // Read only known skip markers privately; never project raw log content.
        let stack_failure = log.stage == "Failed to destroy stack";
        let skip = (log.stage == "Destroy Stack"
            && log.stdout == "Skipping stack destroy, Server is disabled.")
            || (log.stage == "Remove Container"
                && log.stdout == "Skipping container removal, server is disabled.");
        if !log.success || stack_failure {
            failed = true;
            if stages.len() < 8 {
                stages.push(match log.stage.as_str(){"Failed to destroy stack"=>"Failed to destroy stack (Core recorded this as a successful simple log)","Post delete"=>"Post delete","Destroy Stack"=>"Destroy Stack","Remove Container"=>"Remove Container","Remove Service"=>"Remove Service","Remove Container / Service"=>"Remove Container / Service",_=>"Core recorded a failed deletion stage"}.into());
            }
        }
        if skip {
            skipped = true;
            if stages.len() < 8 {
                stages.push("Core skipped runtime cleanup because the Server was disabled".into());
            }
        }
        if matches!(
            log.stage.as_str(),
            "Compose Command"
                | "Remove Container"
                | "Remove Service"
                | "Remove Stack"
                | "Remove Stacks"
        ) && log.success
            && !skip
        {
            command = true;
        }
    }
    let reported = if failed {
        "cleanup_or_post_delete_error_recorded"
    } else if skipped {
        "runtime_cleanup_skipped"
    } else if kind == ResourceKind::Server {
        "server_delete_does_not_request_runtime_cleanup"
    } else if command {
        "cleanup_command_reported_success_not_runtime_proof"
    } else {
        "no_cleanup_stage_corroborated"
    };
    (reported, stages)
}
pub(super) async fn after(
    client: &CoreClient,
    t: &Target,
    before: &History,
) -> Result<(Vec<CleanupUpdate>, bool), AppError> {
    let (items, more) = list(client, t).await?;
    let candidates = items
        .into_iter()
        .filter(|u| !before.ids.contains(&u.id.to_ascii_lowercase()))
        .collect::<Vec<_>>();
    let incomplete = before.incomplete || more || candidates.len() != 1;
    let mut result = vec![];
    for item in candidates.into_iter().take(3) {
        let mut entry = CleanupUpdate {
            id: item.id.clone(),
            attribution: "new_matching_candidate_invocation_not_proven",
            reported_cleanup: "detail_unavailable",
            failed_or_skipped_stages: vec![],
            error: None,
        };
        match client
            .read(GetUpdate {
                id: item.id.clone(),
            })
            .await
        {
            Ok(update)
                if update.id == item.id
                    && target_matches(&update.target, t)
                    && update.operation.to_string() == t.kind.delete_operation() =>
            {
                let (reported, stages) = classify(&update, t.kind);
                entry.reported_cleanup = reported;
                entry.failed_or_skipped_stages = stages;
                if reported == "cleanup_or_post_delete_error_recorded" {
                    entry.error = Some(AppError {
                        message: "Core recorded a cleanup or post-delete error. Definition removal and runtime cleanup are separate; inspect this candidate update when visible. Never automatically repeat deletion.",
                        ..AppError::new(ErrorKind::Upstream, "DeleteCleanup", None)
                    });
                }
            }
            Ok(_) => entry.error = Some(invalid()),
            Err(error) => entry.error = Some(error),
        }
        result.push(entry);
    }
    Ok((result, incomplete))
}
