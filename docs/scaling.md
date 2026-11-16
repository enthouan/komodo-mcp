# Managed Swarm Deployment scaling

Scaling uses an existing Komodo Deployment attached to a Swarm. The typed
tools accept one exact Deployment ID/name (names at most 128 bytes, without
control characters) and a replica count from 0 to 1000.
Zero is an explicit request. They do not scale arbitrary Docker services,
standalone containers or Stacks. Stack replicas belong in Compose source and an
explicit Stack deployment. Templates, ambiguous targets and Server-only
Deployments are rejected without target broadening.

Save and deploy reviews require a current, exact, freshly inspected
replicated service associated with the saved Deployment. Missing service
evidence fails; an empty task cache alone never establishes zero replicas. For initial deployment,
use the existing Deployment definition and `deploy_deployment` workflow first.
Stack-labeled services, global/job modes and conflicting destination names are
unsupported by this scaling workflow.

## Save, deploy and observe separately

1. With `KOMODO_ALLOW_CONFIGURATION_EDIT=true`, call
   `preview_deployment_scale`:

   ```json
   {"target":{"id":"000000000000000000000003"},"replicas":3}
   ```

2. Inspect the current saved/requested counts, changes, target, Swarm and effects.
   Pass the exact same input plus the returned `preview_id` to
   `apply_deployment_scale`. This submits at most one existing typed
   `UpdateDeployment`, followed by an independent full raw configuration
   read-back. `configuration.saved_configuration_verified` must be true before
   treating the save as verified. No-op saves issue no write.

3. With `KOMODO_ALLOW_OPERATE=true`, call
   `review_deployment_scale_deploy` with the saved target/count. This reads and
   binds the complete private configuration, Swarm/Build references and exact
   current service specification/version. It submits nothing. Review its
   recreation and interruption effects.

4. Pass that same target/count and its fresh `preview_id` to
   `deploy_scaled_deployment`. The existing Core `Deploy` path submits at most
   once. Its final Deployment resolution checks the reviewed saved definition
   again. The result retains the ordinary submission receipt and known update
   ID, and separates an unsubmitted/denied/failed/unknown deploy from the saved
   configuration. A partial outcome does not roll back a save.

5. Call `observe_deployment_scale` independently:

   ```json
   {"target":{"id":"000000000000000000000003"},"replicas":3,"timeout_seconds":10}
   ```

   Check `end`, `latest.matching_replica_observations`, current/pending/failed
   tasks, terminating tasks, gaps and coverage. Execution success is a separate
   observation. Verify application health independently. Repeat the reviewed
   save/deploy stages for scale-down or an intentional count of zero.

Saving requires local configuration-edit, Deployment Read/Write, Swarm
Read/Inspect, and attached Build Read when applicable. The deploy stages use
local operate, Deployment Read/Execute, Swarm Read plus Inspect, and attached
Build Read. Their grants are independent: a configuration-only child can save,
then an operate-only child can make a fresh deploy review. Core remains the
permission authority. Observation needs Deployment Read and Swarm Read/Inspect;
it requires neither local mutation grant. There is no permission, credential,
endpoint or target fallback after denial.

## Pinned replica argument and preservation

Periphery 2.3.2's service-create command does not use the saved `replicas` field.
Scaling therefore sets that field and manages only its `--replicas=N` extra
argument. Existing whole `--replicas=N`, whole `--replicas N`, or separate
`--replicas`/`N` forms become one canonical argument in their existing position;
an absent flag is appended. Duplicate flags and compound replica fragments are
rejected. Other accepted argument strings and their order remain unchanged,
including `--replicas-max-per-node` and quoted ordinary values. Explicit global
or job modes, name overrides, shell control/expansion, unbalanced quoting and
complex conflicting forms require deliberate editing with the existing
definition editor before a new scale preview. The bounded recognizer does not
execute a shell or rewrite arbitrary saved arguments.

The partial write sends only `replicas` and `extra_args`. It preserves image,
Build/version, registry references, environment/secrets, mounts, ports, labels,
command, automatic policies and both saved target fields. Preview displays omit
extra arguments and secret-bearing values. Full raw configuration/reference
baselines, including unknown and secret-only fields, stay private; sanitized
display output is never used to reconstruct a definition.

Core `Deploy` removes and recreates a service. This can interrupt workloads and
changes its Docker service ID. It is not an in-place replica update. Saved
arguments remain trusted configuration and can have broader effects. Existing
automatic policies can also consume the saved definition later. Core resolves
image tags, latest Builds, secret interpolation and registry credentials at
execution time; a review does not freeze those external values.

## Evidence, bounds and recovery

All review families share sixteen process-local, ten-minute, single-use records.
Inputs bind the operation, exact target selector and count. A scale-save review
retains the existing definition review privately with its original expiry,
without consuming a second persistent slot. Private configuration/references
are bounded to 256 KiB; arguments use the existing 32-entry/512-byte and 16 KiB
partial-write limits. Bodies are bounded to 2 MiB. Stale or consumed reviews
require a fresh explicit review. Core offers no conditional save/deploy or
transaction: another writer can race after the final checks.

Observation takes at most three samples, scans at most 512 cached tasks per
sample, freshly inspects at most sixteen current/terminating tasks, and returns
within the requested 1–30 second deadline (default five). It performs zero
mutations. Only the freshly identity-checked current service contributes
replicas. Old service IDs and historical shutdown/remove tasks never supply
current replicas; still-running terminating tasks block a match. Unknown task
states, missing counts, scan clipping, fresh-read gaps and sampling caps cannot
establish a match. A service replacement during one observation ends it with
`identity_changed`.

Service counts and task discovery are Core caches without a generation or
completeness guarantee. Matching fresh/cached observations are useful evidence,
not an atomic cluster proof. Empty cache alone is insufficient, especially for
zero. Core's service health describes task desired states; even running tasks
and a Core `Healthy` state do not prove application health.

Preserve a known update ID and use `get_update`/`wait_for_update` after restart.
For a lost acknowledgement, the existing read-only `reconcile_operation` can
search Deployment/whole-deployment/Deploy history, with attribution unconfirmed.
Never replay deployment automatically because a response, observation or health
check timed out. Restart loses private previews; make a fresh deploy review of
the independently saved definition when a new explicit deployment is intended.

The compiled-production-stdio tests use deterministic pinned-wire fixtures;
they do not run Core's permission engine or Docker. Final manager/worker scale,
placement and independent health acceptance belongs to issue #112 and must use
the actual integrated merged-main executable.
