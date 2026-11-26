# Upgrade from v0.9.0 to v1.0.0 and roll back

Start with the **published v0.9.0** installation and an MCP client that can show its initialization response. Use the v1.0.0 procedure once the release and its verification evidence are available. The server keeps its configuration in the launching environment and its resources in Komodo Core. This upgrade requires no Core data conversion, new credentials or changes to existing tool calls. The installation change is the executable or image pin and its reported package version. [Published baseline](https://github.com/enthouan/komodo-mcp/issues/145#issuecomment-7176096008) · [Support policy](stable-support.md)

## Record the working installation

1. Start a new MCP session. Record the release, `initialize` response's `result.serverInfo.version` (expect `0.9.0`), host architecture, client, Core and Periphery versions. Save the results of the [connection checks below](#check-the-connection-after-each-restart) for comparison.
2. Back up the client launch entry or the Compose file, overrides, project name and private env file in operator-only storage. Preserve secret-store references, private CA configuration, Core resource grants, tool allowlist and denylist, and all four `KOMODO_ALLOW_*` settings. For HTTP also preserve the port, reverse proxy, allowed origins and the client's token reference. Keep backups containing credentials private. Record only paths or reference names in shared evidence.
3. The server needs the same `KOMODO_ADDRESS`, `KOMODO_API_KEY` and `KOMODO_API_SECRET` after restart. HTTP clients send their own independent bearer on every request to `/mcp`. The listener matches it against `KOMODO_HTTP_TOKENS`. Keep the Core key pair on the server.

## Native stdio

1. Read the absolute executable path from the client's active stdio entry. Keep the installed v0.9.0 executable and matching archive. Recover the archive from the v0.9.0 release if necessary. Record its filename, target and SHA-256. Verify it using its entry in the release's `SHA256SUMS` and compare the extracted executable with the configured path. The following command should exit zero. [Archive names and checksum commands](distribution.md#verify-and-install)

   ```sh
   cmp "/absolute/path/from/v0.9.0-archive/komodo-mcp" "/absolute/path/from/client/komodo-mcp"
   ```

2. Get `komodo-mcp-v1.0.0-<target>.tar.gz` and the verification records from the stable release. Use the target matching the machine that launches the server. Follow the [stable verification commands](stable-verification.md#verify-a-native-download) to check the archive checksum, provenance, source revision and build identity. Extract into a **new version-specific directory**. Keep the old path available. Record the new absolute executable path and archive digest.
3. Stop the MCP server in your client. Change just the executable path in the saved stdio entry to the new one and relaunch. Keep the same arguments, environment and credential references. Run the connection checks below with expected version `1.0.0`.
4. **Roll back** by stopping the server, restoring the saved client entry with its original v0.9.0 path and environment, and relaunching. Repeat the executable comparison and connection checks. Require the original artifact, `0.9.0`, the original discovery and the same scoped read.

## Compose with authenticated Streamable HTTP

Use the same deployment directory, Compose project, override files and private environment as the existing service. The examples assume its env file is `.env`. Clear any conflicting exported `KOMODO_MCP_IMAGE` before changing that file. Compose gives shell values precedence.

1. Record the effective image reference, the running container's image ID and the matching repository digest. These commands limit inspection to identity fields so credentials stay out of the output.

   ```sh
   service_id=$(docker compose --env-file .env ps -q komodo-mcp)
   docker inspect --format '{{.Config.Image}} {{.Image}}' "$service_id"
   image_id=$(docker inspect --format '{{.Image}}' "$service_id")
   docker image inspect --format '{{json .RepoDigests}}' "$image_id"
   ```

   The published GHCR v0.9.0 index is `sha256:0dac5c0d80dd463c8d4eed87a8538606f2059988eb90862f8404177016ab3b21`. Confirm the running image ID matches the local platform image obtained from that pin. Save both the verified rollback pin and the original configuration.

   ```sh
   previous=ghcr.io/enthouan/komodo-mcp@sha256:0dac5c0d80dd463c8d4eed87a8538606f2059988eb90862f8404177016ab3b21
   docker pull "$previous" && test "$(docker image inspect --format '{{.Id}}' "$previous")" = "$image_id"
   ```

2. Use the [container verification commands](stable-verification.md#verify-the-container-and-its-sbom) and record the verified v1.0.0 **GHCR index digest**. In the private env file set `KOMODO_MCP_IMAGE=ghcr.io/enthouan/komodo-mcp@sha256:<verified-index-hex>`, replacing the placeholder. Keep the same service definition, credentials, mounts and port. Validate the effective pin and recreate just the MCP service.

   ```sh
   docker compose --env-file .env config --images
   docker compose --env-file .env config --quiet &&
     docker compose --env-file .env pull komodo-mcp &&
     docker compose --env-file .env up -d --no-deps --force-recreate --wait komodo-mcp
   ```

3. Repeat the identity inspection from step 1. Require the new digest in the configured reference and compare the running image ID with the ID obtained by inspecting that digest locally. Reconnect the client to the existing endpoint, such as `http://127.0.0.1:3000/mcp`, using its existing bearer. Start a fresh session and run the checks below with expected version `1.0.0`.
4. **Roll back** by restoring the saved Compose configuration and private environment in the same project. Set its `KOMODO_MCP_IMAGE` to the verified v0.9.0 rollback pin above. Repeat the pull, recreate, identity inspection and a fresh authenticated session. Require the recorded platform image ID, a healthy service, `0.9.0`, the original discovery and the same scoped read.

## Check the connection after each restart

1. Initialize a fresh session over the original transport. Check `result.serverInfo.name` is `komodo-mcp` and record `result.serverInfo.version`. During a pre-cut rehearsal, use the candidate's actual Cargo version together with its commit and artifact digest. A healthy HTTP container alone establishes only that the listener answers.
2. Run `tools/list`, following any discovery cursor. Compare the names with the saved selection and local grants. For the read-only triage configuration expect exactly `get_stack`, `list_stacks` and `stack_diagnose`. If a harmless read such as `list_deployments` was excluded before the upgrade, call it directly and confirm the same unknown-tool error.
3. In `tools/call`, submit the following parameters. Choose a scoped test account with a known first visible Stack and record its ID from `result.structuredContent.items`. Expect one matching item before and after the change. A successful empty page can be legitimate for an empty account, but cannot confirm access to your expected Stack.

   ```json
   {"name":"list_stacks","arguments":{"page":0,"page_size":1}}
   ```

If the version, image identity, tool selection or resource access differs, restore the verified installation before resuming operator work. Replacing the server affects future MCP sessions. Changes previously submitted to Core persist. Use the original update ID to [observe an interrupted operation](operator-quickstart.md#recover-an-interrupted-operation). [Troubleshooting](https://komodo-mcp.com/guides/troubleshooting/) · [Support and private security reporting](stable-support.md#getting-help)
