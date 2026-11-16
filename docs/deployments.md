# Typed Deployment definitions

Set `KOMODO_ALLOW_CONFIGURATION_EDIT=true` in the MCP child environment. This
trusted grant permits desired configuration, including executable commands,
mounts, environment, networking and extra container arguments. Core remains the
permission authority. Reading needs Read; saving an existing Deployment needs
Write; new Server/Swarm and Build references need Read plus Attach on that
resource. Creation also needs Core's creation policy to allow the enabled caller.
No credential or permission fallback occurs.

**Image updates require both local grants:** also set `KOMODO_ALLOW_OPERATE=true`
to apply any supplied image, including Build references, pinned digests and
unchanged image inputs. Core can inspect registries and auto-deploy under its
existing policies, and those policies can race. Configuration-only callers can
preview images, create definitions and save ordinary settings or attachments;
direct image apply fails locally before Core reads/writes. Discovery describes
this requirement, previews set `requires_operate_to_apply`, and the shared typed
write transport enforces it independently.

For managed Swarm replica changes, use the [scaling workflow](scaling.md).
Pinned Periphery 2.3.2 ignores the saved replicas field during service creation;
the scaling tools also manage only its replica extra argument, then require a
separate reviewed Deploy and independent task/health observation.

## Create, inspect and edit

Preview creation with an explicit image and existing target:

```json
{"name":"preview_deployment_create","arguments":{"name":"example-deployment","config":{"attachment":{"kind":"server","target":{"name":"example-server"}},"image":{"kind":"image","reference":"busybox:1.37.0"},"network":"bridge","ports":"19090:8080","environment":"TOKEN=[[MY_SECRET_REFERENCE]]\n","command":"sleep 600"}}}
```

Inspect the normalized name, source/target, redacted changes, effects and
`preview_id`. Submit the **exact same** `name` and `config`, plus that token, to
`apply_deployment_create`. Creation performs zero preview writes and at most one
typed CreateDeployment write. Core's container-name normalization preserves
letters' case, digits, `_`, `.` and `-`, replaces other characters with `_`, trims
whitespace and strips leading nonalphanumerics. Empty, overlong and ObjectId-shaped
names are rejected. The preview has no resource ID until Core returns the new
stable ID. Inspect it with `get_deployment`.

With both local grants, preview a one-field desired image update:

```json
{"name":"preview_deployment_update","arguments":{"target":{"id":"000000000000000000000003"},"patch":{"image":{"kind":"image","reference":"busybox:1.36.1"}}}}
```

Call `apply_deployment_update` with the exact same target/patch plus the returned
`preview_id`. Its HTTP partial contains only `image`; it preserves ports,
environment references, registry-account references, target and every omitted
setting. The image is a whole typed variant rather than a nested partial merge.
Use the original values/references for any complete field replacement; display
redaction markers are rejected throughout, including creation names.

## Supported fields and clearing

Both creation's `config` and update's `patch` accept:

| Field | Shape | Omission and explicit clear |
| --- | --- | --- |
| `image` | `{"kind":"image","reference":"registry.example/team/app:v2"}` | Preserve; empty reference/null unsupported |
| `image` | `{"kind":"build","target":{"id":"<Build ID>"},"version":"1.2.3"}` | Resolve existing Build; version required; `0.0.0` explicitly selects latest at deploy |
| `attachment` | `{"kind":"server","target":{"id":"<ID>"}}` or the `swarm` kind | Preserve; detachment/null unsupported; switching kinds clears the opposite target ID |
| `image_registry_account` | String account reference | Preserve; `""` selects Core's default (Build account or none for a direct image) |
| `environment` | Complete environment string | Preserve; `""` clears; nonempty content/references stay redacted |
| `ports` | Complete mapping string, e.g. `19090:8080` | Preserve; `""` clears |
| `volumes` | Complete mapping string, e.g. `/owned/data:/data` | Preserve; `""` clears; values stay redacted |
| `command` | Container command string | Preserve; `""` clears; values stay redacted |
| `network` | Nonempty string, at most 256 bytes | Preserve; empty/null unsupported; Core's creation default is `host`, shown in the preview |
| `extra_args` | At most 32 nonblank strings of 512 bytes each | Preserve; `[]` clears; nonempty values stay redacted |
| `labels` | Complete label string | Preserve; `""` clears; values stay redacted |
| `send_alerts` | Boolean | Preserve; use true/false, no null |
| `replicas` | Integer 0 through 1000, used in Swarm mode | Preserve; zero explicitly records zero replicas for a future deployment |

Selectors require exactly one stable ID or exact name. They use bounded positive
resolution, reject templates/ambiguity, and retain raw referenced definitions
privately. The SDK supports a whole Image or Build variant; Build versions use
exactly three nonnegative integer components, each fitting i32, with no leading
zeros except zero itself. Direct image inputs accept a bounded ASCII subset for
registry/port/path/tag plus an optional `@sha256:` and 64 hex digits, at most 512
bytes. URLs, embedded user/password syntax, shell syntax and unsupported clears
are rejected. This validates the typed input, not remote image availability.

All nulls and unknown fields fail closed. The complete patch and its canonical
partial body must fit within 16384 JSON bytes. Pinned SDK deserializers normalize
mapping, label, environment and argument strings before comparison/submission,
including final newlines. Only supplied fields and deliberate opposite-target
clears are serialized; no reconstructed full definition or unrelated defaults
are sent. Custom-name edits are outside this field set: Core can rename a live
container before saving them. Existing custom/deployed names stay intact.

## Saved configuration and runtime observations

`definition.details.before`/`after` show bounded desired image/Build, target,
network and policy metadata. Environment, commands, mounts, arguments, labels
and registry-account references are never echoed as raw changed values. Legacy
credential-bearing image URLs are hidden. Configured API credentials are removed
**before** display clipping; changed fields flag `before_truncated` and, when
needed, `after_truncated`. Private comparisons and write bodies retain complete
values.

`runtime_observation` is separately labeled as **preview-time Core inventory**.
Core 2.3.2 can fill its reported image from desired configuration when container
or service cache data is missing. Accordingly `image_may_be_desired_fallback`
is true and `running_configuration_verified` stays false. A saved image, a Core
reported image, successful update acknowledgment and actual running configuration
are distinct evidence. Use explicit workload operations and independent runtime
inspection to establish what runs and whether it is healthy.

MCP submits no deploy, restart, pull, shell or provisioning request from these
definition tools. Core itself refreshes target caches. A submitted direct image
also schedules CheckDeploymentForUpdate: Periphery may log in to a registry and
inspect manifests using [`docker buildx imagetools inspect`](https://docs.docker.com/reference/cli/docker/buildx/imagetools/inspect/). This inspection does
not pull image layers. When its saved auto-update policy permits and a running
workload has a newer digest, **Core can auto-deploy**, which can pull/recreate the
workload. Hardcoded digests skip that registry check in Core 2.3.2. Both local
grants remain required for image apply regardless of digest or previewed policy.

Build updates save a reference/version without starting a Build. `0.0.0` follows
the latest Build version when deployment runs; future builds can change it.
Core's existing `redeploy_on_build` policy can redeploy running Deployments after
an attached build completes, including pinned versions. Saving does not establish
registry-tag immutability. This surface preserves existing auto-update,
polling and build-redeploy policies and does not enable them on creation.

The SDK warns about cleanup after target reassignment. Inspected **Core 2.3.2**
UpdateDeployment hooks refresh the new target and perform no old-target removal;
old containers/services can remain. Inspect both targets. The live fixture checks
that reassignment to a disabled pre-provisioned Server and back preserves the old
running container's ID, image and running state.

## Shared contract and evidence limits

Deployment tools use the [same preview/apply contract](configuration.md) as Stack
tools: one private store shared across resource types, 16 pending previews per
process, ten-minute expiry, single use and no survival across restart. Tokens
bind the operation/resource kind, exact input, stable identity and authoritative
configuration, including hidden secrets and referenced target/Build definitions.
A token cannot cross resource types even if ID and patch are identical. Apply
consumes it before I/O and rechecks baselines/references. Stale proposals and
unchanged edits submit zero writes. Image apply still requires both grants for
an unchanged image input.

Creation rechecks only bounded **visible** name absence. Core enforces global
uniqueness and authorization; hidden names and a concurrent creator can reject
the write. Core offers no conditional write/create or invocation idempotency.
Another writer can race between final reads and submission. A mutable latest
Build selection or registry tag is not frozen by this preview.

One authenticated write uses the shared 15-second deadline, 2 MiB response cap,
disabled redirects and disabled mutation retries. A lost, failed, malformed,
oversized or mismatched reply yields `submission_unknown`; the token remains
consumed. Inspect saved configuration before considering a fresh explicit edit.
`saved_configuration_verified` means the typed response reports the intended
fields on the expected identity/name, with no runtime or connectivity claim.
The raw reply must contain the submitted fields; SDK defaults cannot synthesize
missing-field verification.

Deterministic tests use the compiled production binary over stdio:

```sh
cargo test --locked --test configuration --test deployment_configuration --test capabilities
```

They cover direct images, latest/pinned Build versions and both Server/Swarm
targets; all four grant profiles; serialized one-field bodies; omission/clear;
raw-secret/reference staleness; cross-resource token rejection; redaction before
clipping; no-ops; upstream denial; mutation deadline/response uncertainty; and
zero execution submissions. The [disposable runner](operations-walkthrough.md#opt-in-disposable-coreperiphery-smoke)
adds Core/Periphery 2.3.2 direct-image Server creation, authoritative preservation,
local image denial, actual desired-versus-running image separation and Server
reassignment. Build references and Swarm targets have fixture coverage only.

Pinned implementation sources:
[write API](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/api/write/deployment.rs),
[config/image schema](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/deployment.rs),
[Core handler](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/write/deployment.rs),
[resource hooks](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/resource/deployment.rs),
[build redeploy hook](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/execute/build.rs),
and [Periphery manifest inspection](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/docker/image.rs).
