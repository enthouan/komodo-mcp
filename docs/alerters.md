# Alerter configuration and explicit test delivery

Use `list_alerters` and `get_alerter` to inspect visible named Alerters. They
require Core Read and return enabled status, endpoint variant, bounded type and
resource filters, and maintenance clock metadata. URLs, interpolation expressions,
credentials, Ntfy email, maintenance names/descriptions, provider bodies and raw
configuration/history output are omitted. A projection is never an apply baseline.

The pinned Core/Periphery contract is 2.3.2. Five endpoint variants are supported:
Custom, Slack, Discord, Ntfy and Pushover. Core interpolates ordinary variables and
configured/variable secrets in all five URLs **at send time**. MCP neither resolves
those values nor returns their expressions. Ntfy email is a literal `X-Email`
recipient; Core does not interpolate it.

## Preview and save

Creation requires configuration-edit and an enabled Core administrator:

```json
{"name":"owned-local-receiver","config":{"endpoint":{"kind":"custom","url":"[[OWNED_RECEIVER_URL]]"},"alert_types":["Test"]}}
```

Call `preview_alerter_create` with that object, review its safe fields/effects,
then pass the identical name/config and returned `preview_id` to
`apply_alerter_create`. The endpoint and its original URL/reference are required.
No SDK endpoint default is accepted. `enabled` defaults **false**; explicitly
creating enabled also requires operate. Use only an authorized recipient.

Existing updates use `preview_alerter_update` then `apply_alerter_update`:

```json
{"target":{"id":"REPLACE_WITH_24_HEX_ID"},"patch":{"enabled":false,"alert_types":["Test"],"resources":[],"except_resources":[],"maintenance_windows":[]}}
```

Both local configuration-edit **and operate** grants are required for every
existing-definition apply, including disabling or a no-op. Core resource Write
remains authoritative; an ordinary identity with Write may update without admin.
Changing endpoint/routing/maintenance settings can alter future automatic
notifications even when a preflight observed disabled. **Saving never calls
TestAlerter or SendAlert.** It does not cancel deliveries already in flight.

Omit any field to preserve its original private value. `null` is invalid. Empty
`alert_types` and `resources` mean unrestricted inclusion; exclusions still apply.
Empty `except_resources` removes exclusions. Empty `maintenance_windows` clears
all windows. Filters are explicit `{"kind":"system"}` or
`{"kind":"resource","resource_kind":"server","target":{"id":"..."}}` entries;
select at most 16 per list. Supplied resource selectors are positively resolved
under the same caller and rebound at apply; no missing target implies global scope.

An endpoint patch needs `kind` and may omit `url` only when preserving the same
variant. Switching variants requires the original new URL/reference. Ntfy email
uses `{"kind":"set","value":"owned@example.test"}` or `{"kind":"clear"}`;
omission preserves it. Unrelated patches never send an endpoint replacement.
Unknown saved config/endpoint fields remain in private baselines and read-back;
replacing an endpoint containing unsupported fields is refused rather than
silently discarding them. No displayed redaction marker can be submitted as a value.

A maintenance window contains `name`, optional `description`, a typed `schedule`,
`hour`, `minute`, `duration_minutes`, optional `timezone` and explicit `enabled`.
Schedules are `{"kind":"daily"}`, `{"kind":"weekly","day_of_week":"Monday"}`,
or `{"kind":"one_time","date":"2026-11-16"}`. MCP bounds lists to 16, validates
calendar dates and clock ranges, and accepts durations 1–1439 minutes. Core alone
evaluates timezones and activity. Empty timezone inherits Core configuration.
Pinned Core wraps time-of-day arithmetic, including across midnight; **1440 is
not an all-day window**, and these tools never claim active suppression from a
configuration projection. Inspect receiver evidence independently.

All configuration families share 16 process-local previews, ten-minute expiry
and single use. Alerter previews bind complete raw saved resources, including
secret-only/reference changes, plus supplied filter identities. Creation also
binds caller authority and name absence. Apply rechecks, detects no-op and submits
at most one typed write. It validates the raw acknowledgement before SDK defaults
and separately reads the entire saved configuration. `saved` records a valid
acknowledgement; `saved_configuration_verified` reports independent read-back.
A lost/malformed reply stays `submission_unknown`. No automatic retry, credential
fallback, rollback or atomic compare-and-swap is provided. Another administrator
can change configuration after the last read or Core can reread it later.
Inputs are capped at 16 KiB, private saved resources at 64 KiB and raw HTTP bodies
at 2 MiB. Lists use positive bounded Core pagination; projections disclose truncation.

## Test one saved target

Enable operate separately, then deliberately call:

```json
{"target":{"id":"REPLACE_WITH_24_HEX_ID"},"acknowledge_delivery":true}
```

`test_alerter` positively resolves exactly one non-template saved Alerter, rechecks
it inside the shared execution boundary and submits one stable-ID `TestAlerter`.
Core Execute is required; local configuration-edit is unnecessary. There is no
broadcast `SendAlert` tool, no automatic enabling and no automatic test after save.
A final configuration race remains: Core uses the saved endpoint at execution.

Keep three observations separate:

| Evidence | Meaning |
| --- | --- |
| Accepted receipt / `update_id` | Core acknowledged this target and operation |
| `observed_alerter_test.core_report` | Core reports pending, success or failure |
| Owned HTTP receiver record | Independently observed delivery; not inferred by MCP |

Disabled Alerters produce failed Core updates without sending. Tests bypass
alert-type and resource filters, but **honor maintenance**. A maintenance-suppressed
test can have a successful Update without any provider attempt. A failed Update
may reflect permissions, disabled state or a provider failure. Therefore
`provider_attempt` remains `not_established` and
`independently_observed_receipt` remains `not_verified`, including Core success.
Provider HTTP acceptance itself does not prove a downstream human received anything.

Use `get_update` / bounded `wait_for_update` with a known Update ID. They retain
safe status and the same delivery limitation across restarts. An acknowledged ID
survives denied/failed history reads. If the acknowledgement was lost, use read-only
`reconcile_operation` with target kind `alerter`, requested scope
`{"kind":"alerter_test"}`, operation `test` and the retained submission time window.
Candidates remain unattributed and never authorize a resend. There is no test
cancellation or delivery rollback.

Compiled-stdio tests use an authenticated deterministic pinned-wire fixture to
cover grants, variants, preservation, stale private previews, denial, malformed or
lost replies, bounds, restart history and no implicit sends. They do not establish
actual provider delivery. Milestone integration #112 runs the production executable
from actual integrated merged main against an owned local HTTP receiver and
records independent request counts, disabled/failed/suppressed behavior and cleanup.
