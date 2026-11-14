# Core-owned Action and Procedure schedules

`list_schedules` returns positive bounded pages of Core schedule owners. Core
includes Actions and Procedures with **nonempty expressions**, even when disabled,
and includes templates. Empty expressions do not appear. Core collects and sorts
matching owners internally before pagination; MCP bounds its wire requests and
output, rather than Core's internal database work.

Use `page_size` from 1 through 100 (default 30), keep it unchanged, and continue
with `next_page`. The lookahead is not an atomic inventory snapshot. Each item
includes owner kind/ID/name, expression, format, timezone, enabled state and any
Core-reported next run, last run or schedule error. The listing does not expose
source, stages, default arguments or secrets. Schedule-alert policy is unavailable
in this listing and appears as null; `get_schedule` reads it from the parent.

```json
{"owner_kind":"action","target":{"name":"trusted-action"}}
```

`get_schedule` also inspects empty schedules and templates. `saved` describes the
owning resource; `observation` separately reports a Core `ListSchedules` lookup.
The lookup uses Core's literal name filter and checks the exact owner kind/ID and
expected fields, with at most four pages of 100 results. Missing owners, exhausted
search bounds, denied/unavailable reads and mismatching fields remain explicit
evidence gaps. A same-named Action and Procedure are distinct owners. Templates
remain readable but cannot be mutated through these tools.

## Saved fields and cached execution evidence

Core's next-run timestamp and schedule error come from its scheduler cache. The
API supplies no cache generation or atomic relationship to the saved definition.
A matching row means its reported fields agree; it does not prove a fresh cache
generation, a particular scheduled execution or workload health. Preserve reported
past timestamps as cache evidence instead of replacing them with MCP-clock guesses.

Core's `last_run_at` is the **start time of the latest completed run**, whether
successful or failed, manual or scheduled. Null can mean unavailable evidence;
it is not proof of no history. It does not establish scheduled/caller attribution.

Core stores invalid expressions/timezones and reports parser/next-occurrence
errors through the cache. An apply can therefore return `outcome=saved` and
`saved_configuration_verified=true` alongside a Core schedule error. Configuration
save verification, cache-read errors and Core parsing errors are distinct fields.
Disabled or empty schedules are not parsed; no error while disabled proves no
syntax validity. A disabled owner may still report stale cache data.

Cron uses Core's parser with required seconds; for example, `0 0 9 * * *` means
09:00 in the effective timezone. English expressions such as `every day at 9:00 am`
use Core's English conversion; Core keeps its first six Cron fields. MCP does not
parse, normalize or calculate the next occurrence of either format.

An explicit timezone is an IANA zone interpreted by Core. Empty timezone inherits
Core's configured timezone, or its host's local timezone if that is also empty.
It never uses the MCP host's clock or timezone. The listing does not identify that
inherited zone; its returned UTC-millisecond timestamp remains Core evidence.

## Preview and apply

Set configuration-edit for preview; apply also requires operate for every nonempty
patch, plus Core Read/Write on the owner. Local grants remain independent and Core
permissions remain authoritative. No privileged credentials are borrowed. Core
administrator creation rules for new Actions remain unchanged; these tools update
existing owners only. A schedule save delegates future execution to Core's
administrator Action/Procedure identities. Actions also retain Core-host Deno
`--allow-all` access. Treat scheduling as trusted future execution.

`preview_schedule_update` and `apply_schedule_update` accept:

| Patch field | Meaning |
| --- | --- |
| `expression` | Original Core expression, at most 1024 UTF-8 bytes; `""` deliberately clears it |
| `format` | `cron` or `english`, mapped to Core's formats |
| `timezone` | Explicit zone, at most 128 bytes; `""` inherits Core defaults |
| `enabled` | Saved schedule switch |
| `alert` | Core schedule-run alert policy |

Nulls, unknown fields and control characters are rejected. Omission preserves
existing fields. Source, stages, arguments, startup/webhook settings and secrets
cannot enter schedule patches. Generic Action/Procedure definition tools continue
to reject scheduling fields. The shared mutation transport independently requires
both grants for any actual owner update, so direct SDK paths cannot bypass effects.

Example preview:

```json
{
  "owner_kind":"procedure",
  "target":{"name":"trusted-procedure"},
  "patch":{
    "expression":"0 0 9 * * *",
    "format":"cron",
    "timezone":"America/New_York",
    "enabled":true,
    "alert":false
  }
}
```

Apply the identical input with the returned `preview_id`. The shared bounded,
ten-minute, single-use store binds the owner kind, operation, input and entire
private parent configuration. Apply consumes the preview, rechecks the baseline,
sends one typed `UpdateAction` or `UpdateProcedure` with only supplied schedule
fields, verifies the echoed fields, then independently reads back the complete
saved parent. Concurrent private source/default/stage/secret changes invalidate
the preview. Empty patches can return a write-free no-op with edit alone; supplied
unchanged fields also submit zero writes when both grants are available.

Cache observation follows a save or no-op and cannot erase the save acknowledgement.
`matches_expected_fields` compares a cache listing row with the inspected/proposed
fields. After an uncertain write it does not establish who wrote matching values.
A missing/failed cache lookup is reported without retrying a write. Core provides
no compare-and-swap, so changes can race the final read.

Disabling or clearing changes the saved policy and Core's cache, without cancelling
already dispatched/running work. Core reschedules after execution using a captured
owner definition; an in-flight run can race a later edit and reinsert older cache
state. Inspect saved policy and cache separately. No atomic stop, rollback or child
cancellation is promised. Explicit resource cancellation remains in the
[Action](actions.md#explicit-cancellation) and [Procedure](procedures.md) tools.
MCP runs no background schedule executor.

## Verification

Deterministic production-stdio fixtures use the same pinned `croner` 3.0.1,
`english-to-cron` 0.1.7, `chrono` 0.4.45 and `chrono-tz` 0.10.4 versions as Core
2.3.2, with a fixed **fixture Core** clock. These are development dependencies.
Tests cover both owner types, before/after US spring and autumn DST offsets,
explicit/inherited zones, English, invalid expressions/timezones, enable/update/
disable/clear, private-parent preservation, denial/staleness, uncertain saves and
cached/missing/mismatching evidence. They do not replace #93's integrated live
acceptance against real pinned Core/Periphery.

Pinned references: [Core listing](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/schedule.rs),
[Core scheduler/parser/timezones](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/schedule.rs),
[last completed run lookup](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/helpers/query.rs),
and [literal resource term filters](https://github.com/moghtech/komodo/blob/v2.3.2/client/core/rs/src/entities/resource.rs).
