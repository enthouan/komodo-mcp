# Explicit Docker category pruning

`preview_docker_prune` and `apply_docker_prune` operate on one positively
resolved, enabled, non-template Komodo Server. Both require
`KOMODO_ALLOW_PRUNE=true` **and** `KOMODO_ALLOW_OPERATE=true` at startup, plus
an enabled Core identity with **Execute** on that Server. Configuration-edit
and administrator privileges are unnecessary. The opt-in defaults to false;
operate or configuration-edit alone never exposes either tool. Disabled routes
are rejected before argument handling or Core I/O, and the same policy is
checked again in the narrow typed transport.

Only these five explicit categories are supported:

| Category | Pinned Core/Periphery 2.3.2 effect |
| --- | --- |
| `image` | Remove every unused image eligible for all-unused pruning, including tagged images without a container reference. This is broader than dangling-image cleanup. |
| `container` | Remove stopped containers on the selected daemon. |
| `network` | Remove networks Docker considers unused, under its current network/reference rules. |
| `volume` | Remove unused local volumes, including **named** and anonymous volumes. Unused named volumes can hold application data. |
| `system` | Remove stopped containers, unused networks, all unused images, build cache and eligible unused **anonymous volumes**. This includes volume effects; it does not establish removal of unused named volumes. |

The system/volume distinction is also documented by
[Docker system prune](https://docs.docker.com/reference/cli/docker/system/prune/)
and [Docker volume prune](https://docs.docker.com/reference/cli/docker/volume/prune/).
Docker's installed version and current references decide eligibility. These are
category-wide operations, not deletion of one named image, container, network
or volume. There is no fleet loop, caller-supplied filter/flag, builder/buildx
prune tool, schedule or automatic cleanup. System pruning's build-cache effects
remain part of its real scope.

## Preview, acknowledge, submit once

```json
{"name":"preview_docker_prune","arguments":{"target":{"name":"owned-test-server"},"category":"volume"}}
```

Read the returned stable Server ID, category, destructive effect and evidence
limits. Each selector requires exactly one original stable `id` or exact
case-sensitive `name`. Missing, ambiguous, invalid or unsupported targets and
categories fail closed; none implies system or all-Server scope.

```json
{"name":"apply_docker_prune","arguments":{"target":{"name":"owned-test-server"},"category":"volume","preview_id":"<returned token>","acknowledge_destructive_effects":true}}
```

Apply must repeat the exact preview target/category and explicitly acknowledge
the destructive scope. A missing/false acknowledgement submits nothing. Reviews
share the process-local 16-entry, ten-minute, single-use store. They bind the
complete raw Server/configuration/connection state, current caller and Core
permission response privately, within 256 KiB and the shared 16384-byte input
limit. No raw private baseline, credential or baseline hash is displayed.
Restart invalidates old tokens. Apply revalidates once in the handler and again
inside the final transport immediately before at most one typed execute request.
Authority loss or private connection/reference drift prevents submission.

## Inventory is supporting evidence

The preview uses the existing image/network/volume inspection projections and
a bounded cached container count. System previews collect all four categories.
Responses are capped at 2 MiB and counts at 512, with explicit incomplete/error
fields. These local limits do not bound Core/Docker's internal enumeration.
No fresh inspect permission is substituted for Core Execute.

Core has no exact atomic dry run. Its cache has no generation timestamp or
completeness guarantee: an empty list can also mean absent cache or a suppressed
poll failure. A cached `in_use` value does not establish current eligibility.
Unavailable supporting inventory is reported without choosing another target
or category. It does not manufacture an empty prune set.

Inventory is deliberately not frozen into an object-set promise. Another
container can stop, a volume can become unused, or an image can be pulled after
preview. An explicitly acknowledged prune still submits when the old cached
count was zero. The final Server check cannot atomically bind a physical Docker
daemon, live connection or eventual object set; Core offers no compare-and-swap
or expected-daemon parameter. Use only a deliberately selected engine whose
entire category-wide effect is authorized.

## Follow the receipt without replay

`prune_submissions` counts the single attempted Core submission. The nested
receipt separates `accepted`, `rejected` and `unknown`, preserves a valid Core
update ID when returned, and performs only one bounded follow-up read. Core can
acknowledge an update that later fails because it is busy, denied or its
Periphery/daemon is unavailable. Successful Core completion and independently
verified removal/preservation are separate facts. There is no rollback.

```json
{"name":"wait_for_update","arguments":{"id":"<known update ID>","timeout_seconds":30}}
```

Read-only history, wait and reconciliation remain available after disabling
pruning/operate or restarting the process. Retain the stable Server, category
and local submission window before applying. If the reply is lost, use the
same explicit context:

```json
{"name":"reconcile_operation","arguments":{"target":{"kind":"server","id":"000000000000000000000001"},"requested_scope":{"kind":"docker_prune","category":"volume"},"operation":"prune"}}
```

Recovery filters the actual Server and category-specific Core operation.
Candidates remain unconfirmed: a history match does not attribute the request,
and no candidates does not prove it was never accepted. Neither recovery nor a
timeout automatically repeats pruning. A new explicit apply can repeat effects;
inspect current state and make a deliberate new decision first.

Prune history omits raw object names, registry references, command/output/error
payloads and revision observations, including after restart. Safe stage outcomes
and update identity remain available. Core/Periphery may retain their own raw
logs; these tools do not erase them. The opt-in controls this direct tool surface;
it does not sandbox trusted Actions or other saved automation with equivalent
upstream authority.

Compiled-stdio tests use authenticated deterministic wire/fault fixtures, not
a Docker permission engine. Actual category/system removals, preservation of
in-use fixtures and submission counts are mandatory in #112 on owned isolated
engines using the final integrated actual-main executable. Never run those
acceptance prunes on a shared development or production daemon.
