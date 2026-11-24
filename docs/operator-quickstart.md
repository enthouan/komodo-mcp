# First read, stack diagnosis and recovery

Use this walkthrough with a native stdio client or an authenticated Streamable
HTTP client. The Stack and update IDs in the examples come from the
[synthetic incident fixture](../tests/fixtures/core-2.3.2/INCIDENT.md).
Use the IDs returned by your own Core when working with your resources.

## Choose the Core scope and client credential

Create an enabled, non-admin Core service account. Give it Read access to the
Stacks and their Servers that this client should see. Grant Inspect or Logs
where Core requires those permissions for service state or log evidence. Check
inherited access from
groups, resource base permissions and resource-type grants as well as the
explicit Stack grant. One account per environment lets you keep staging and
production inventory separate. See [effective permissions](access.md#inspect-direct-and-effective-access).

| Connection | What the MCP client needs | What the server needs |
| --- | --- | --- |
| Native stdio | A client configuration that launches the executable with its private environment. Access to the local process controls access to this connection. | The Core URL in `KOMODO_ADDRESS` and the account's `KOMODO_API_KEY` and `KOMODO_API_SECRET` in that process's environment. |
| Streamable HTTP | The server's `/mcp` URL and its own inbound token. Send `Authorization: Bearer <your-inbound-token>` on each request. Use HTTPS when connecting across a network. | The same Core variables, `KOMODO_TRANSPORT=http`, and random client tokens in `KOMODO_HTTP_TOKENS`. Keep the Core key and secret on the server. |

Every inbound HTTP token on an instance uses that instance's Core account,
resource permissions and local tool selection. Give each client its own token.
Use separate instances with separately scoped Core accounts when callers need
different resource access. A session ID refers to protocol state. Requests
that carry it still require the bearer token.

## Discover and diagnose with three tools

Add this entry to the **server's environment** and leave all four
`KOMODO_ALLOW_*` grants at their default of `false`.

```sh
KOMODO_ENABLED_TOOLS=list_stacks,get_stack,stack_diagnose
```

Restart the server and reconnect. After MCP initialization, use the client's
tool list (`tools/list`). With Core 2.3.2, stdio and authenticated HTTP both returned
`["get_stack","list_stacks","stack_diagnose"]` when the names in `result.tools[].name`
were sorted. This view omits the schemas from the full response.
[Recorded discovery on both transports](https://github.com/enthouan/komodo-mcp/issues/134#issuecomment-7119966070)
This setting narrows available actions. The Core account controls which Stacks
those actions can see.

1. Call `list_stacks` with the arguments below. Inspect `structuredContent.items`
   for the Stack's name, state and stable `id`. If needed, follow `next_page`
   using the same `page_size` until it is null. An empty result can mean the
   account has no visible Stacks.

   ```json
   {"name":"list_stacks","arguments":{"page":0,"page_size":10}}
   ```

2. Copy the exact `id` from the selected item into `get_stack`. Check its
   name and target before continuing. This example uses the ID of the fixture's
   `alpha-web` Stack.

   ```json
   {"name":"get_stack","arguments":{"id":"000000000000000000000002"}}
   ```

3. Pass **that same returned ID** to `stack_diagnose`. Report the outcome, the
   affected services and any gaps. The fixture reports an unhealthy `web`
   service and cites a failed rollout. On your own Core, missing observations
   can leave the diagnosis inconclusive.

   ```json
   {"name":"stack_diagnose","arguments":{"id":"000000000000000000000002"}}
   ```

4. Check that the selection also applies to direct calls. In a client that can
   send a raw MCP request, call the excluded read tool `list_deployments` with
   empty arguments.

   ```json
   {"name":"list_deployments","arguments":{}}
   ```

   Expect an unknown-tool error. The MCP server rejects this request before
   contacting Core. In the disposable fixture, compare Core's request count
   immediately before and after this call. The count stays the same.

Each JSON example is the `params` object for an MCP `tools/call`. In a client
with a tool form, choose the named tool and enter only its `arguments` fields.
[Stack diagnosis](diagnostics.md#stack-diagnosis) explains the bounds and evidence.

## Staging deployment

Keep the three-tool instance for triage. For an explicitly approved staging
deployment, use a separate instance with a staging-scoped Core account that
has Execute on the chosen Stack. Set `KOMODO_ALLOW_OPERATE=true` and select the
following tools.

```sh
KOMODO_ENABLED_TOOLS=list_stacks,get_stack,stack_diagnose,deploy_stack,get_update,wait_for_update,reconcile_operation
```

Before submitting, retain the stable target ID, action, exact scope and UTC
start time. Resolve the owned staging Stack with `list_stacks` and `get_stack`.
For HTTP, connect to the staging instance using its staging bearer token.
With approval for that exact target and scope, submit once. This example
deploys the whole fixture Stack.

```json
{"name":"deploy_stack","arguments":{"target":{"id":"000000000000000000000002"},"scope":{"kind":"whole_stack"}}}
```

Save the Core update ID from the receipt as soon as it arrives. Observe it with
`wait_for_update` and inspect the Stack with `stack_diagnose` after completion.
Core execution success and current workload health are separate observations.

## Recover an interrupted operation

After a disconnect, either recovery path uses reads. A fresh client with
access to the same Core history and the required read tools can follow it.

**Known update ID.** Look up that exact ID with `get_update`, or wait for it
with `wait_for_update`. If the wait ends before completion, resume with the
same ID. After the update completes, check workload health independently.

```json
{"name":"wait_for_update","arguments":{"id":"100000000000000000000001","timeout_seconds":10}}
```

**Lost receipt and unknown update ID.** Record the observed end time. Use
`reconcile_operation` with the original target, action, scope and known time
bounds in UTC Unix milliseconds. The example uses a whole-Stack deploy.

```json
{"name":"reconcile_operation","arguments":{"target":{"kind":"stack","id":"000000000000000000000002"},"requested_scope":{"kind":"whole_stack"},"operation":"deploy","submission_window":{"started_ts":1735689600000,"ended_ts":1735689605000}}}
```

Zero candidates leaves acceptance unknown. A single candidate or multiple
candidates still have unconfirmed attribution. Check candidate IDs with read
tools and retain any history gaps. Reissuing `deploy_stack` would submit another
operation. See [reconciliation outcomes](operations.md#recover-an-interrupted-submission-without-repeating-execution).
