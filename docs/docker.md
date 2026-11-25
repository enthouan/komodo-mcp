# Inspect Docker objects on one Server

All nine tools are available in the read-only profile. Select one existing,
non-template Komodo Server by stable resource ID or exact name. Every tool
requires **Server Read**. **`get_container` also requires Server Inspect**.
To expose only the container pair, set
`KOMODO_ENABLED_TOOLS=list_containers,get_container` at startup. To expose
cached discovery alone, set `KOMODO_ENABLED_TOOLS=list_containers`. The denylist
`KOMODO_DISABLED_TOOLS` takes precedence over the allowlist.

## Investigate a stopped container

In an MCP client connected over stdio or authenticated Streamable HTTP, call
`list_containers` with your Server name. These are MCP `tools/call` parameters.

```json
{"name":"list_containers","arguments":{"target":{"name":"test-server"}}}
```

The default page shows containers whose cached state is `running`. Include
stopped containers to see all states. Follow `data.next_page` with the same
Server, filter and page size until it is null.

```json
{"name":"list_containers","arguments":{"target":{"name":"test-server"},"include_stopped":true,"page":0,"page_size":10}}
```

Find the exited container by name, then copy its entire `identity` object from
the result into `get_container`. The IDs below illustrate the shape.

```json
{"name":"get_container","arguments":{"identity":{"server_id":"000000000000000000000001","container_id":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}}
```

The cached summaries contain the Server-bound container identity, name, image
and state where available. A null identity means the cache lacked a usable full Docker ID or the
identity contained a configured credential and was withheld. An empty page can reflect the running filter, the page
offset, an empty or missing cache, or a suppressed Periphery poll failure.
`observed_count` counts the received rows across all states before filtering.

Fresh inspection addresses only that Server and full container ID. It verifies
the Docker response ID before returning name, state, exit code, health status
and restart count. Unavailable fields are null. If Docker reports that exact ID is
missing, `get_container` returns `not_found`. List again to discover a same-name replacement.
A different or missing ID in a successful response produces `invalid_response`.
A caller with Server Read alone can list but receives `permission_denied` on
inspection. Environment, command arguments, labels, host configuration, health
logs and raw Docker errors are omitted.

## Inspect images, networks and volumes

```json
{"name":"list_images","arguments":{"target":{"name":"test-server"},"page_size":30}}
{"name":"get_image","arguments":{"target":{"id":"000000000000000000000001"},"object":{"name":"test/web:latest"}}}
{"name":"get_image_history","arguments":{"target":{"name":"test-server"},"object":{"id":"sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa"},"page_size":30}}
{"name":"get_network","arguments":{"target":{"name":"test-server"},"object":{"name":"test-network"}}}
{"name":"get_volume","arguments":{"target":{"name":"test-server"},"object":{"name":"test-volume"}}}
```

IDs are synthetic; use identifiers from your own inventory. `list_networks` and
`list_volumes` use the same target and pagination arguments as `list_images`.
In pinned Core 2.3.2, Server Read suffices for fresh image, network and volume
inspection and image history. Local grants never confer Core permissions; denial never
switches identity, Server or endpoint. No removal, prune, terminal, creation or
other mutation endpoint is used.

Object selectors contain exactly one `id` or `name`; nulls, wildcards, ID prefixes
and credential-bearing references are rejected. Image IDs are full lowercase
`sha256:` content digests; network IDs are either 64 lowercase hexadecimal characters (local networks) or
full 25-character lowercase alphanumeric Swarm IDs (overlay networks).
Image names match an exact cached primary name, tag or manifest digest reference.
Bracketed IPv6 registry addresses, with optional ports, are supported.
Network and volume names are exact and case-sensitive. Network selectors accept
bounded nonblank names, including underscores, colons and spaces; control
characters, wildcard forms and credential-bearing names are rejected. Public
network names that are unsafe, unusually formatted or oversized are omitted with
`name_omitted: true`; their full IDs remain usable, and they cannot invalidate the
rest of an inventory. Volume selectors retain Docker volume-name syntax. Multiple images or
networks with the same name require an explicit ID. Duplicate identities are an
invalid response. Volumes use their exact name because Core's list has no
immutable volume identifier.

Image, network and volume details resolve the complete bounded cached selection, request its exact ID
(volume name), then verify the returned identity. Name selection also checks the
fresh name or image references. Missing, ambiguous, changed, malformed or locally
incomplete selectors fail without widening scope. A same-name volume replacement
cannot be excluded: Docker/Core provide no incarnation token in this list.
Image history uses the verified content ID, so retagging cannot redirect it to a
different image. Separate requests are not an atomic snapshot.

## Evidence and sensitive fields

`source: "core_cache"` identifies lists. Core returns empty lists for missing
Docker cache; Periphery can suppress individual poll failures to empty vectors.
No cache generation timestamp or completeness guarantee is available. An empty
list describes the received cache and cannot prove Docker has no objects. A
cache miss during exact selection returns `not_found` with that limitation.
The selected Server's cached state, enabled flag and infrastructure-error
presence remain visible without exposing the raw error or connection config.

`source: "fresh_inspect"` identifies details and history. Core refuses fresh
inspection when its cached Server state is NotOk or Disabled; this is reported
as `unreachable` with a Server-specific explanation. Exact supported Docker
missing-object errors bound to the submitted identity report `not_found`.
Permission denial, malformed responses, explicit unsupported endpoint responses,
transport failures and oversized bodies remain distinct failures. Unrecognized
provider/proxy errors are safely classified as upstream errors, not absence.

Images expose content ID, up to four safe tags and digest references each, size,
creation time, architecture/OS and layer count where available. Cached use is
separate from fresh details. Unknown platform values are `other`; invalid or
oversized references are omitted with `references_incomplete`. Image history
preserves layer order and exposes only index, valid content ID (null for Docker's
`<missing>`), Unix creation timestamp and known nonnegative size. History commands,
comments and tags are omitted completely, including apparently harmless ones.

Networks expose full ID, name, known driver class, scope, creation time and
IPv6/internal/attachable/ingress flags. Fresh details include attachment count,
without raw container names, addresses, labels or IPAM settings. Volumes expose
name, known driver class, scope, creation time and known size/reference count.
Unknown drivers are `other`; negative/unavailable sizes and counts stay null.
Cached `in_use` is never promoted to a fresh detail observation.

Field projections omit image environment, labels, commands, authors, comments,
descriptors/URLs/annotations, build-history commands, network and volume driver
options, IPAM metadata, volume mountpoints/status, cluster secret references and
plugin contexts. Raw errors never leave the transport. Known configured API
credentials are also redacted before display clipping. User-selected identifiers
and syntactically valid image references remain operational metadata; do not put
secret values in names or tags. No raw Docker object is returned.

## Bounds and cleanup decisions

Upstream bodies are capped at 2 MiB before decoding, including calls through the
shared typed client. Lists and history scan at most 512 rows and return local
pages of 1–100 items, default 30. Follow `next_page` with the same page size.
`observed_count` and `scan_incomplete` disclose local clipping; a complete local
scan does not establish upstream cache coverage. Image, network and volume detail resolution refuses
a cache larger than 512 rows. Container inspection addresses its full ID directly.
Responses are capped at 96 KiB after safe projection
and configured-credential redaction; request a smaller page if that budget fails.

With an ordinary Server name or stable Komodo ID, lists require at most three
Core reads, container inspection three, other details four and history five.
ObjectId-shaped Server names can need
three extra bounded lookup pages. Each request has a 15-second deadline, with a
five-second connect timeout. Client-side pagination and response limits do not
bound Core's or Docker's internal enumeration. Pages are not snapshot tokens.

Inventory supports deliberate cleanup decisions; it is **not an authoritative
prune dry run**. Cached `in_use`, sizes and reference counts do not freeze
concurrent Docker activity or prove eligibility. These tools cannot remove or
prune objects. Separately enabled pruning is tracked in #35.

## Validation and sources

`cargo test --locked --test docker` exercises the compiled executable against an owned
loopback Core wire fixture. Container discovery, list-to-inspect calls, denied
inspection, stale identity and per-tool selection run over both stdio and
authenticated Streamable HTTP. It covers every local
capability profile, exact/missing/ambiguous/stale selection, Read-only permission
requirements, unsafe metadata, unavailable/unsupported endpoints, clipped caches,
oversized bodies and shared-transport bounds, with measured zero `/write` and
`/execute` requests and child/mock cleanup. This does not run Core's permission
engine or Docker. The packaged-candidate regression is tracked in
[#48](https://github.com/enthouan/komodo-mcp/issues/48).

Behavior was verified against tagged v2.3.2 [Core Docker resolvers](https://github.com/moghtech/komodo/blob/v2.3.2/bin/core/src/api/read/docker.rs),
[container conversion and inspection](https://github.com/moghtech/komodo/blob/cc1c5aa5dabb738e843c47f5e87fc040648c2bb7/bin/periphery/src/docker/container.rs),
[image conversion and history](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/docker/image.rs),
[network conversion](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/docker/network.rs),
[volume conversion](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/docker/volume.rs)
and [poll suppression](https://github.com/moghtech/komodo/blob/v2.3.2/bin/periphery/src/api/poll.rs).
Exact Docker/Bollard missing-error formats follow [image errors](https://github.com/moby/moby/blob/v28.0.0/daemon/images/image.go),
[network errors](https://github.com/moby/moby/blob/v28.0.0/libnetwork/error.go)
and [volume errors](https://github.com/moby/moby/blob/v28.0.0/volume/service/errors.go);
other formats stay conservative upstream failures.

Network selection/display rules were also checked against the upstream
[network-name validator](https://github.com/moby/libnetwork/blob/master/config/config.go),
and IPv6 reference syntax against the official
[Docker reference grammar](https://github.com/distribution/reference/blob/main/regexp.go).
