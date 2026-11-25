#!/usr/bin/env python3
"""Exercise one verified packaged candidate in a fresh disposable Core/Periphery project."""
import argparse
import asyncio
from contextlib import asynccontextmanager
import hashlib
import json
from pathlib import Path
import platform
import re
import secrets
import subprocess
import tempfile

import disposable_core_smoke as smoke
import disposable_resource_sync_smoke as sync
from candidate_clients import HttpMcp, NativeMcp
from container_release import sha_file
from distribution_build import native_platform
from release_artifacts import Packages


CORE_WORKLOADS = {
    "Stack": ("mcp-smoke-stack",),
    "Deployment": ("mcp-smoke-deployment",),
    "ResourceSync": ("mcp-regression-stale",),
    "Server": ("mcp-smoke-server", "mcp-smoke-alternate"),
}
WORKLOAD_RESOURCES = (*CORE_WORKLOADS, "service_users", "nested_fixture_containers",
                      "nested_deployment_containers", "outer_candidate_containers")
REDACTION_SURFACES = ("MCP frames", "protocol errors", "process diagnostics", "scenario evidence")
DENIAL_TOOLS = ("apply_resource_sync", "get_resource_sync_pending", "refresh_resource_sync_pending",
                "review_resource_sync_pending")
CLEANUP_STEPS = {"start", "outer candidate containers", "stop Periphery", "verify service users", "delete service user",
                 "list deployed container", "delete deployed container", "verify fixture container", "delete fixture container",
                 *(verb + " " + name for verb in ("verify", "delete")
                   for name in ("ResourceSync","Deployment","Stack","alternate Server","bootstrap Server"))}


def expect_clean(value):
    encoded = json.dumps(value)
    smoke.require(all(secret not in encoded for secret in smoke.SECRETS if secret),
                  "regression output disclosed a credential or synthetic private value")


def verify_contract_identity(record, checkout):
    path = "docs/contracts/v1.json"
    contract = record.get("contract")
    if not isinstance(contract, dict):
        raise ValueError("missing frozen contract identity")
    baseline, revision = contract.get("baseline_commit"), record["distribution"]["revision"]
    digest = contract.get("sha256")
    if (contract.get("path") != path or not isinstance(digest, str) or not re.fullmatch(r"[a-f0-9]{64}", digest)
            or not all(isinstance(commit,str) and re.fullmatch(r"[a-f0-9]{40}", commit) for commit in (baseline, revision))):
        raise ValueError("invalid frozen contract identity")
    git = ["git", "-C", checkout]
    ancestry = subprocess.run([*git,"merge-base","--is-ancestor",baseline,revision],capture_output=True)
    if ancestry.returncode:
        raise ValueError("contract baseline is not a known ancestor of the packaged revision")
    for commit in {baseline, revision}:
        blob = subprocess.run([*git,"cat-file","blob",commit+":"+path],capture_output=True)
        if blob.returncode or hashlib.sha256(blob.stdout).hexdigest() != digest:
            raise ValueError("frozen contract does not match the recorded bytes at both revisions")
    return baseline


def history(admin, kind, resource_id, operation=None):
    query = {"target.type": kind, "target.id": resource_id}
    if operation is not None:
        query["operation"] = operation
    entries = admin.read("ListUpdates", {"query": query, "page": 0})
    smoke.require(entries.get("next_page") is None, "owned execution history exceeded the first page")
    results = {}
    for entry in entries["updates"]:
        smoke.require(entry["target"] == {"type": kind, "id": resource_id}
                      and (operation is None or entry["operation"] == operation), "Core history escaped its scope")
        identifier = smoke.identity(entry)
        smoke.require(identifier not in results, "duplicate authoritative history ID")
        results[identifier] = entry
    return results


def nested(*args):
    return smoke.run([*smoke.COMPOSE, "exec", "-T", "docker", "docker", *args])


def native_host_platform():
    arch = {"arm64":"arm64", "aarch64":"arm64", "x86_64":"amd64"}[platform.machine()]
    return f"{platform.system().lower()}/{arch}"


def grant(admin, user_id, kind, target_id, level, specific=()):
    admin.write("UpdatePermissionOnTarget", {
        "user_target": {"type": "User", "id": user_id},
        "resource_target": {"type": kind, "id": target_id},
        "permission": {"level": level, "specific": list(specific)},
    })


class Candidate:
    def __init__(self, args):
        self.args = args
        self.record = json.loads(args.candidate_record.read_bytes())
        manifest = self.record["distribution"]
        self.version, self.revision = manifest["version"], manifest["revision"]
        Packages.from_receipt(self.record, self.revision, self.version)
        self.baseline = verify_contract_identity(self.record, smoke.WORKSPACE)
        self.ref = "komodo-mcp-candidate:git-" + self.revision
        self.owns_image = False
        self.owned = {}
        self.outcome = {}
        self.context = None
        self.nested_ids = []

    def prepare(self):
        args = self.args
        smoke.REPORT.update(package_version=self.version, source_commit=self.revision,
                            contract_baseline_commit=self.baseline,
                            candidate_record_sha256=sha_file(args.candidate_record),
                            transport=args.transport,
                            scenarios=self.outcome, workload_cleanup_status="not_started",
                            workload_cleanup_verified=False)
        if args.transport == "stdio":
            digest = sha_file(args.binary)
            matches = [f for f in self.record["distribution"]["files"]
                       if f["kind"] == "native" and f["executable_sha256"] == digest]
            smoke.require(len(matches) == 1, "executable differs from the native artifact record")
            native_platform(matches[0]["target"])
            smoke.REPORT.update(target=matches[0]["target"], platform=native_host_platform(), executable_sha256=digest)
            return args.binary
        receipt = self.record["container"]
        smoke.require(sha_file(args.oci_archive) == receipt["archive"]["sha256"],
                      "downloaded OCI archive differs from shared candidate record")
        self.platform = receipt["platforms"][args.platform]
        engine = smoke.run([*smoke.DOCKER, "version", "--format", "{{.Server.Os}}/{{.Server.Arch}}"])
        smoke.require(engine == args.platform, "select the native Docker engine platform")
        existing = smoke.run([*smoke.DOCKER, "image", "ls", "--quiet", self.ref])
        if not existing:
            smoke.run([*smoke.DOCKER, "image", "load", "--input", str(args.oci_archive)], timeout=300)
            self.owns_image = True
        self.verify_image()
        smoke.REPORT.update(target=args.platform, platform=engine, index_digest=receipt["index_digest"],
                            platform_manifest_digest=self.platform["manifest_digest"],
                            executable_sha256=self.platform["executable_sha256"])
        return None

    def verify_image(self):
        output = smoke.run([*smoke.DOCKER, "image", "inspect", "--platform", self.args.platform,
                            "--format", "{{json .}}", self.ref])
        loaded = json.loads(output)
        manifest, config = self.platform["manifest_digest"], self.platform["config_digest"]
        index = self.record["container"]["index_digest"]
        descriptor = (loaded.get("Descriptor") or {}).get("digest")
        if descriptor:
            exact = (descriptor == manifest and loaded["Id"] in (index, manifest, config)
                     or descriptor == index and loaded["Id"] in (manifest, config))
        else:
            exact = loaded["Id"] == config
        smoke.require(exact and f"{loaded['Os']}/{loaded['Architecture']}" == self.args.platform,
                      "loaded image configuration or platform differs from candidate")

    @asynccontextmanager
    async def connect(self, identity, **settings):
        credentials = {k:self.context[identity][k] for k in ("key", "secret")}
        if self.args.transport == "stdio":
            async with await NativeMcp.start(binary=self.args.binary,
                    executable_sha256=smoke.REPORT["executable_sha256"], version=self.version,
                    address=self.context["address"], credentials=credentials, environment=settings) as client:
                smoke.REPORT.update(mcp_version=client.version, client=client.client_info)
                yield client
            return
        bearer = secrets.token_urlsafe(36)
        smoke.SECRETS.append(bearer)
        environment = dict(settings, KOMODO_API_KEY=credentials["key"], KOMODO_API_SECRET=credentials["secret"],
            KOMODO_ADDRESS="http://core:9120", KOMODO_TRANSPORT="http", KOMODO_HTTP_BIND="0.0.0.0:3000",
            KOMODO_HTTP_TOKENS=bearer, RUST_LOG="trace")
        project, owner = smoke.PROJECT, smoke.OWNER
        network = project + "_default"
        smoke.require(smoke.run([*smoke.DOCKER, "network", "inspect", "--format",
            '{{index .Labels "komodo-mcp.test.session"}}', network]) == owner,
            "MCP network does not belong to this disposable fixture")
        # Supply private credentials through a mode-0600 file. Docker command
        # diagnostics and the retained regression record contain no environment.
        with tempfile.NamedTemporaryFile(mode="w", dir=smoke.ROOT) as env:
            for key, value in environment.items():
                smoke.require(not any(ch in value for ch in "\n\r\x00"), "invalid fixture environment")
                env.write(f"{key}={value}\n")
            env.flush()
            cid = smoke.run([*smoke.DOCKER, "create", "--pull", "never", "--platform", self.args.platform,
                "--network", network, "--env-file", env.name, "--publish", "127.0.0.1::3000",
                "--read-only", "--cap-drop", "ALL", "--security-opt", "no-new-privileges",
                "--label", "komodo-mcp.test.session=" + owner,
                "--label", "komodo-mcp.test.role=packaged-candidate", self.ref])
        smoke.require(re.fullmatch(r"[0-9a-f]{64}", cid), "missing owned MCP container ID")
        self.owned[cid] = owner
        try:
            identity = json.loads(smoke.run([*smoke.DOCKER, "inspect", "--format", "{{json .}}", cid]))
            selected = (identity.get("ImageManifestDescriptor") or {}).get("digest")
            manifest, config = self.platform["manifest_digest"], self.platform["config_digest"]
            if selected:
                exact = selected == manifest and identity["Image"] in (
                    self.record["container"]["index_digest"], manifest, config)
            else:
                exact = identity["Image"] == config
            smoke.require(exact, "container image changed before start")
            executable = smoke.ROOT / (cid + ".bin")
            smoke.run([*smoke.DOCKER, "cp", cid + ":/usr/local/bin/komodo-mcp", str(executable)])
            smoke.require(sha_file(executable) == self.platform["executable_sha256"],
                          "running container executable differs from matching native artifact")
            executable.unlink()
            smoke.run([*smoke.DOCKER, "start", cid])
            smoke.poll(lambda: smoke.run([*smoke.DOCKER, "exec", cid, "/usr/local/bin/komodo-mcp",
                                          "--healthcheck"]) or True, "owned HTTP MCP readiness", 60)
            mapping = smoke.run([*smoke.DOCKER, "port", cid, "3000/tcp"])
            smoke.require(re.fullmatch(r"127\.0\.0\.1:\d+", mapping), "unexpected HTTP MCP binding")
            async with await HttpMcp.start(endpoint=f"http://{mapping}/mcp", bearer=bearer,
                                               version=self.version) as client:
                smoke.REPORT.update(mcp_version=client.version, client=client.client_info)
                async def interrupt():
                    smoke.run([*smoke.DOCKER, "kill", cid])
                    client.closed = True
                client.interrupt = interrupt
                yield client
        finally:
            self.drop_container(cid)

    def candidate_containers(self):
        return smoke.run([*smoke.DOCKER, "ps", "-aq", "--no-trunc",
            "--filter", "label=komodo-mcp.test.session="+smoke.OWNER,
            "--filter", "label=komodo-mcp.test.role=packaged-candidate"]).splitlines()

    def drop_container(self, cid):
        owner = smoke.run([*smoke.DOCKER, "inspect", "--format",
                           '{{index .Config.Labels "komodo-mcp.test.session"}}', cid])
        smoke.require(owner == smoke.OWNER == self.owned[cid], "fixture MCP container owner changed")
        try:
            expect_clean(smoke.run([*smoke.DOCKER, "logs", cid]))
        finally:
            smoke.run([*smoke.DOCKER, "rm", "--force", cid])
            del self.owned[cid]

    def final_image_cleanup(self):
        if self.owns_image:
            self.verify_image()
            smoke.require(not smoke.run([*smoke.DOCKER, "ps", "-aq", "--filter", "ancestor="+self.ref]),
                          "candidate image is still in use")
            smoke.run([*smoke.DOCKER, "image", "rm", self.ref])
            self.owns_image = False

    def seed_containers(self):
        smoke.require(not nested("ps", "-aq"), "expected a fresh private Docker daemon")
        image = "busybox:1.37.0"
        nested("pull", image)
        smoke.REPORT["workload_image"] = nested("image", "inspect", "--format",
                                               "{{index .RepoDigests 0}}", image)
        running = nested("create", "--name", "mcp-regression-running", "--label",
                         "komodo-mcp.test.session=" + smoke.OWNER, image, "sleep", "600")
        smoke.require(re.fullmatch(r"[a-f0-9]{64}", running), "invalid owned running container ID")
        self.nested_ids.append(running)
        stopped = nested("create", "--name", "mcp-regression-stopped", "--label",
                         "komodo-mcp.test.session=" + smoke.OWNER, "--label", "private="+self.marker,
                         "--env", "FIXTURE_SECRET="+self.marker, "--health-interval", "1s",
                         "--health-cmd", 'echo "$FIXTURE_SECRET"; exit 1', image,
                         "sh", "-c", "while [ ! -f /tmp/exit-fixture ]; do sleep 1; done; exit 17",
                         self.marker, "fixture")
        smoke.require(re.fullmatch(r"[a-f0-9]{64}", stopped), "invalid owned stopped container ID")
        self.nested_ids.append(stopped)
        nested("start", running, stopped)
        def unhealthy_probe():
            health = json.loads(nested("inspect", "--format", "{{json .State.Health}}", stopped))
            return health and health["Status"] == "unhealthy" and any(
                row["ExitCode"] != 0 and self.marker in row["Output"] for row in health["Log"])
        smoke.poll(unhealthy_probe, "owned unhealthy probe with private output")
        nested("exec", stopped, "touch", "/tmp/exit-fixture")
        smoke.poll(lambda: nested("inspect", "--format", "{{.State.Status}}", stopped) == "exited",
                   "owned diagnostic container exit")
        # Establish that the field-redaction check actually exercises secret-bearing evidence.
        raw = json.loads(nested("inspect", stopped))[0]
        smoke.require(raw["State"]["ExitCode"] == 17 and raw["Config"]["Labels"]["private"] == self.marker
                      and "FIXTURE_SECRET="+self.marker in raw["Config"]["Env"] and self.marker in raw["Args"]
                      and any(self.marker in row["Output"] for row in raw["State"]["Health"]["Log"]),
                      "seeded private metadata or failed health-check output missing")
        return running, stopped, raw["State"]["Health"]["Status"], raw["RestartCount"]

    async def exercise_containers(self, running, stopped, expected_health, expected_restarts):
        ctx = self.context
        target = {"id":ctx["server_id"]}
        admin = ctx["provisioner"]
        smoke.poll(lambda: {row.get("id") for row in admin.read("ListContainers", {"server":target["id"]})
                           if row["state"] == "exited"} >= {stopped}, "Core cache saw stopped container")
        async with self.connect("reader") as peer:
            tools = {t["name"] for t in (await peer.rpc("tools/list", {}))["tools"]}
            smoke.require({"list_containers","get_container"} <= tools and "deploy_deployment" not in tools,
                          "read-only capability discovery differed")
            page = await peer.tool("list_containers", {"target":target})
            rows = page["data"]["items"]
            smoke.require([r["identity"]["container_id"] for r in rows] == [running]
                          and page["source"] == "core_cache", "unexpected running inventory")
            rows, page_number = [], 0
            while True:
                page = await peer.tool("list_containers", {"target":target, "include_stopped":True,
                                                          "page_size":1, "page":page_number})
                rows.extend(page["data"]["items"])
                if page["data"]["next_page"] is None:
                    break
                smoke.require(page["data"]["next_page"] == page_number+1 < 4, "unexpected next page")
                page_number += 1
            stopped_rows = [r for r in rows if r["state"] == "exited"]
            smoke.require(len(rows) == 2 and len(stopped_rows) == 1 and page_number == 1,
                          "expected two one-container pages")
            selected = stopped_rows[0]["identity"]
            smoke.require(selected == {"server_id":target["id"],"container_id":stopped},
                          "MCP and Docker container identities differ")
            detail = await peer.tool("get_container", {"identity":selected})
            smoke.require(detail["source"] == "fresh_inspect" and detail["data"]["state"] == "exited"
                          and detail["data"]["exit_code"] == 17 and detail["data"]["identity"] == selected
                          and detail["data"]["health_status"] == expected_health
                          and detail["data"]["restart_count"] == expected_restarts,
                          "fresh stopped diagnostics differ from Docker")
        async with self.connect("editor", KOMODO_ENABLED_TOOLS="list_containers,get_container") as peer:
            tools = {t["name"] for t in (await peer.rpc("tools/list", {}))["tools"]}
            smoke.require(tools == {"list_containers","get_container"}, "tool selection differed")
            await peer.tool("list_containers", {"target":target})
            await sync.raw_error(peer, "get_container", {"identity":selected}, "permission_denied")
            await sync.raw_error(peer, "get_stack", {"id":smoke.REPORT["scope"]["stack_id"]})
        self.outcome["container_workflow"] = {
            "default_running":1, "all_states":2, "pages":page_number+1, "stopped_exit_code":17,
            "state":detail["data"]["state"], "health":detail["data"]["health_status"],
            "restart_count":detail["data"]["restart_count"], "matched_docker_id":True,
            "list_only_identity":"Server Read + Attach", "inspect_denied":True,
            "hidden_call_rejected":True, "seeded_private_health_output_verified":True,
        }

    async def exercise_configuration(self):
        ctx = self.context
        admin = ctx["provisioner"]
        sid = smoke.REPORT["scope"]["stack_id"]
        before = admin.read("GetStack", {"stack":sid})["config"]
        args = {"target":{"id":sid}, "patch":{"auto_pull":not before["auto_pull"]}}
        async with self.connect("editor", KOMODO_ALLOW_CONFIGURATION_EDIT="true") as peer:
            preview = await peer.tool("preview_stack_update", args)
            smoke.require(preview["writes_submitted"] == 0 and
                          admin.read("GetStack", {"stack":sid})["config"] == before,
                          "configuration preview changed the owned stack")
            saved = await peer.tool("apply_stack_update", dict(args,preview_id=preview["preview_id"]))
            smoke.require(saved["outcome"] == "saved" and saved["writes_submitted"] == 1
                          and admin.read("GetStack", {"stack":sid})["config"] == dict(before,**args["patch"]),
                          "scoped configuration apply differed from Core read-back")
        self.outcome["configuration"] = {"preview_writes":0,"apply_writes":1,
            "field":"Stack.auto_pull", "checked_all_other_config":True}

        # ResourceSync creation is a provisioning-only, admin operation. No admin keys reach MCP.
        sync_name = "mcp-regression-stale"
        source = sync.server_doc("mcp-smoke-alternate", prune=True)
        raw = admin.write("CreateResourceSync", {"name":sync_name,"config":{
            "file_contents":source, "managed":False, "delete":False, "webhook_enabled":False,
            "include_resources":True, "include_variables":False, "include_user_groups":False,
            "pending_alert":False}})
        rid = smoke.identity(raw)
        self.sync_id = rid
        grant(admin, ctx["users"][2]["id"], "ResourceSync", rid, "Write")
        grant(admin, ctx["users"][1]["id"], "ResourceSync", rid, "Read")
        grant(admin, ctx["users"][2]["id"], "Server", ctx["alternate_server_id"], "Write")
        grant(admin, ctx["users"][1]["id"], "Server", ctx["alternate_server_id"], "Read")
        before = history(admin, "ResourceSync", rid, "RunSync")
        untouched = admin.read("GetServer", {"server":ctx["alternate_server_id"]})["config"]
        async with self.connect("editor", KOMODO_ALLOW_OPERATE="true", KOMODO_ALLOW_CONFIGURATION_EDIT="true") as peer:
            await peer.tool("refresh_resource_sync_pending", {"id":rid})
            await self.exercise_denied_apply(rid)
            reviewed = await sync.review(peer, rid, sync.whole())
            admin.write("UpdateResourceSync", {"id":rid,"config":{"file_contents":
                sync.server_doc("mcp-smoke-alternate", prune=False) + "# owned drift\n"}})
            attempt = await peer.tool("apply_resource_sync", {
                "review_reference":reviewed["review_reference"],"binding":reviewed["apply_binding"]})
            smoke.require(attempt["outcome"] == "not_submitted" and attempt["review_consumed"]
                          and attempt["run_sync_requests_submitted"] == 0
                          and attempt["rejection"]["kind"] == "stale_preview",
                          "changed inline source did not invalidate the reviewed plan")
        smoke.require(before == history(admin, "ResourceSync", rid, "RunSync")
                      and admin.read("GetServer", {"server":ctx["alternate_server_id"]})["config"] == untouched,
                      "stale review changed Core history or target configuration")
        self.outcome["stale_resource_sync"] = {"source":"inline", "rejection":attempt["rejection"]["kind"],
            "preflight_refreshes":attempt["preflight"]["refresh_requests_submitted"],
            "run_sync_history_delta":0, "unchanged_server_configuration":True}

    async def exercise_denied_apply(self, rid):
        admin = self.context["provisioner"]
        server = {"server":self.context["alternate_server_id"]}
        before = history(admin, "ResourceSync", rid)
        unchanged = admin.read("GetServer", server)["config"]
        async with self.connect("reader", KOMODO_ALLOW_OPERATE="true", KOMODO_ALLOW_CONFIGURATION_EDIT="true",
                                KOMODO_ENABLED_TOOLS=",".join(DENIAL_TOOLS)) as peer:
            discovery = await peer.rpc("tools/list", {})
            names = sorted(tool["name"] for tool in discovery["tools"])
            smoke.require(names == list(DENIAL_TOOLS) and not discovery.get("nextCursor"),
                          "enabled operation and read tools differ from discovery")
            cached = await peer.tool("get_resource_sync_pending", {"id":rid})
            smoke.require(cached["sync"]["id"] == rid, "read-only caller did not see the owned sync")
            review = await sync.review(peer, rid, sync.whole())
            attempt = await peer.tool("apply_resource_sync", {"review_reference":review["review_reference"],
                                                              "binding":review["apply_binding"]})
        preflight = attempt["preflight"]
        smoke.require(attempt["outcome"] == "not_submitted" and attempt["run_sync_requests_submitted"] == 0
                      and preflight["refresh_requests_submitted"] == 1
                      and preflight["error"]["kind"] == "permission_denied",
                      "read-only Core grant did not deny the preflight refresh before RunSync")
        after = history(admin, "ResourceSync", rid)
        run_before = {key:row for key,row in before.items() if row["operation"] == "RunSync"}
        run_after = {key:row for key,row in after.items() if row["operation"] == "RunSync"}
        smoke.require(run_before == run_after and admin.read("GetServer", server)["config"] == unchanged,
                      "denied preflight changed RunSync history or the managed Server")
        self.outcome["read_only_operation"] = {
            "resource_grant":"Read", "local_operate":True, "local_configuration_edit":True,
            "discovered_tools":names, "visible_sync_read":True, "preflight_write":"RefreshResourceSyncPending",
            "refresh_attempts":preflight["refresh_requests_submitted"], "preflight_error":preflight["error"]["kind"],
            "apply_outcome":attempt["outcome"], "run_sync_requests_submitted":attempt["run_sync_requests_submitted"],
            "core_run_sync_history_delta":len(run_after)-len(run_before), "target_unchanged":True,
            "other_target_updates":[{"id":key,"operation":after[key]["operation"]} for key in sorted(after.keys()-before.keys())],
        }

    async def exercise_recovery(self):
        ctx = self.context
        admin = ctx["provisioner"]
        deployment = smoke.REPORT["scope"]["deployment_id"]
        before = history(admin, "Deployment", deployment, "Deploy")
        async with self.connect("operator", KOMODO_ALLOW_OPERATE="true") as peer:
            receipt = await peer.tool("deploy_deployment", {"id":deployment})
            known = receipt["update_id"]
            smoke.require(receipt["submission"] == "accepted" and re.fullmatch(r"[a-f0-9]{24}",known)
                          and not receipt["automatic_resubmission"], "missing accepted deployment update")
            self.outcome["interruption"] = {"accepted_update_id":known}
            smoke.persist()
            await peer.interrupt()  # Abruptly terminate the observing process after acknowledgement.
        async with self.connect("reader", KOMODO_ENABLED_TOOLS="get_update,wait_for_update") as peer:
            final = await smoke.observe(peer, known)
            smoke.require(final["outcome"] == "succeeded" and final["id"] == known,
                          "read-only reconnection did not verify the accepted deployment")
        after = history(admin, "Deployment", deployment, "Deploy")
        smoke.require(after.keys()-before.keys() == {known} and after[known]["success"]
                      and after[known]["operator"] == ctx["users"][0]["id"],
                      "authoritative Core history did not contain exactly one scoped submission")
        runtime = nested("inspect", "--format", "{{.Id}} {{.State.Running}}", "mcp-smoke-deployment").split()
        smoke.require(len(runtime) == 2 and re.fullmatch(r"[a-f0-9]{64}",runtime[0]) and runtime[1] == "true",
                      "deployed container was not independently observed running")
        self.deployed_id = runtime[0]
        self.outcome["interruption"].update(final_outcome=final["outcome"], matched_operator=True,
            core_deploy_history_delta=1, reconnected_read_only=True, observed_running_via_docker=True)

    def exercise(self, context):
        self.context = context
        services = {}
        for service in ("core", "periphery", "docker", "mongo"):
            cid = smoke.run([*smoke.COMPOSE, "ps", "-q", service])
            smoke.require(re.fullmatch(r"[a-f0-9]{64}", cid), "expected one owned fixture service")
            image = smoke.run([*smoke.DOCKER, "inspect", "--format", "{{.Image}}", cid])
            smoke.require(re.fullmatch(r"sha256:[a-f0-9]{64}", image), "invalid fixture image digest")
            services[service] = image
        smoke.REPORT["fixture_image_ids"] = services
        self.marker = "fixture-private-candidate-" + secrets.token_hex(12)
        smoke.SECRETS.append(self.marker)
        initial_diagnostics = self.seed_containers()
        asyncio.run(self.exercise_containers(*initial_diagnostics))
        smoke.persist()
        asyncio.run(self.exercise_configuration())
        smoke.persist()
        asyncio.run(self.exercise_recovery())
        smoke.persist()
        expect_clean(self.outcome)
        smoke.REPORT["redaction_checked"] = list(REDACTION_SURFACES)

    def remove_workloads(self):
        # The shared runner always removes the private Core database and nested daemon.
        # Also remove and list workload objects while both authoritative services exist.
        if self.args.transport == "http":
            self.cleanup_step = "outer candidate containers"
            # Recover a Docker create whose reply was interrupted.
            for cid in self.candidate_containers():
                self.owned.setdefault(cid, smoke.OWNER)
            for cid in list(self.owned):
                self.drop_container(cid)
        if self.context:
            admin = self.context["provisioner"]
            scope = smoke.REPORT["scope"]
            expected = [("Stack", "mcp-smoke-stack", scope["stack_id"]),
                        ("Deployment", "mcp-smoke-deployment", scope["deployment_id"])]
            if getattr(self, "sync_id", None):
                expected.insert(0, ("ResourceSync", "mcp-regression-stale", self.sync_id))
            for kind, name, ident in expected:
                self.cleanup_step = "verify " + kind
                arg = "sync" if kind == "ResourceSync" else kind.lower()
                current = admin.read("Get" + kind, {arg:ident})
                smoke.require(smoke.identity(current) == ident and current["name"] == name,
                              "owned Core identity changed before cleanup")
                self.cleanup_step = "delete " + kind
                if kind == "Deployment":
                    # Core waits for the container's 10s termination grace. The
                    # ordinary 10s API deadline can expire before its reply.
                    admin.request("/write", "DeleteDeployment", {"id":ident}, timeout=45)
                else:
                    admin.write("Delete" + kind, {"id":ident})
            # Stop the fixture agent before deleting its bootstrap Server.
            self.cleanup_step = "stop Periphery"
            smoke.run([*smoke.COMPOSE, "stop", "periphery"])
            for name, ident in [("mcp-smoke-alternate", scope["alternate_server_id"]),
                                ("mcp-smoke-server", scope["server_id"])]:
                role = "alternate Server" if name == "mcp-smoke-alternate" else "bootstrap Server"
                self.cleanup_step = "verify " + role
                current = admin.read("GetServer", {"server":ident})
                smoke.require(smoke.identity(current) == ident and current["name"] == name,
                              "server replaced before cleanup")
                self.cleanup_step = "delete " + role
                admin.write("DeleteServer", {"id":ident})
            self.cleanup_step = "verify service users"
            users = {entry["id"]:entry["username"] for entry in self.context["users"]}
            current_users = {smoke.identity(user):user["username"] for user in admin.read("ListUsers")}
            smoke.require(all(current_users.get(i) == name for i,name in users.items()),
                          "scoped service user identity changed")
            for ident in users:
                self.cleanup_step = "delete service user"
                admin.write("DeleteUser", {"user":ident})
            self.cleanup_step = "list deployed container"
            deployed = nested("ps", "-aq", "--no-trunc", "--filter","name=^/mcp-smoke-deployment$").splitlines()
            if deployed and deployed == [getattr(self, "deployed_id", None)]:
                self.cleanup_step = "delete deployed container"
                nested("rm", "-f", deployed[0])
            for cid in self.nested_ids:
                self.cleanup_step = "verify fixture container"
                owner = nested("inspect", "--format", '{{index .Config.Labels "komodo-mcp.test.session"}}', cid)
                smoke.require(owner == smoke.OWNER, "nested container ownership changed")
                self.cleanup_step = "delete fixture container"
                nested("rm", "-f", cid)

    def observe_cleanup(self):
        remaining = dict.fromkeys(WORKLOAD_RESOURCES)
        smoke.REPORT["owned_workload_cleanup"] = remaining

        def observe(kind, query, *args):
            # Keep observing independent resources if one source is unavailable.
            try:
                remaining[kind] = query(*args)
            except Exception:
                remaining[kind] = None

        observe("outer_candidate_containers", self.candidate_containers)
        observe("nested_fixture_containers", lambda: nested("ps", "-aq", "--filter",
                "label=komodo-mcp.test.session="+smoke.OWNER).splitlines())
        observe("nested_deployment_containers", lambda: nested("ps", "-aq", "--filter",
                "name=^/mcp-smoke-deployment$").splitlines())
        if self.context:
            admin = self.context["provisioner"]

            def listed(kind, names):
                rows = admin.read("List" + kind + "s", {
                    "query":{"names":list(names)}, "limit":10, "page":0})
                smoke.require(isinstance(rows, list) and len(rows) < 10
                              and all(r["name"] in names for r in rows),
                              "incomplete or unexpected Core cleanup inventory")
                return [row["name"] for row in rows]

            for kind, names in CORE_WORKLOADS.items():
                observe(kind, listed, kind, names)
            users = {entry["id"] for entry in self.context["users"]}
            observe("service_users", lambda: [smoke.identity(user) for user in admin.read("ListUsers")
                                              if smoke.identity(user) in users])
        return remaining

    def cleanup(self):
        report = smoke.REPORT
        report.update(workload_cleanup_status="failed", workload_cleanup_verified=False)
        self.cleanup_step = "start"
        removed = False
        try:
            self.remove_workloads()
            removed = True
        except Exception as error:
            report["failed_workload_cleanup_step"] = self.cleanup_step
            print("Workload cleanup failed during " + self.cleanup_step, flush=True)
            if isinstance(error, smoke.CoreHttpFailure):
                report["workload_cleanup_http_status"] = error.status
            raise
        finally:
            # These are the last authoritative observations before Compose tears down
            # the database and nested daemon. A failed removal still gets a listing.
            remaining = self.observe_cleanup()
            if removed:
                if any(remaining.values()):
                    report["workload_cleanup_status"] = "residue"
                elif any(rows is None for rows in remaining.values()):
                    report["workload_cleanup_status"] = "unknown"
                else:
                    expect_clean(report)
                    report.update(workload_cleanup_status="verified", workload_cleanup_verified=True)
        smoke.require(report["workload_cleanup_verified"], "workload cleanup could not verify zero residue")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--candidate-record", type=Path, required=True)
    parser.add_argument("--transport", choices=("stdio","http"), required=True)
    parser.add_argument("--binary", type=Path, help="extracted native executable for stdio")
    parser.add_argument("--oci-archive", type=Path, help="downloaded candidate.oci.tar for HTTP")
    parser.add_argument("--platform", choices=("linux/amd64","linux/arm64"), help="native Docker engine platform")
    parser.add_argument("--docker-context", required=True)
    parser.add_argument("--project", type=smoke.project_name, required=True)
    parser.add_argument("--evidence", type=Path, required=True)
    args = parser.parse_args(argv)
    if args.transport == "stdio":
        if not args.binary or args.oci_archive or args.platform:
            parser.error("stdio requires --binary")
        args.binary = args.binary.resolve(strict=True)
    elif args.binary or not (args.oci_archive and args.platform):
        parser.error("HTTP requires --oci-archive and --platform")
    candidate = Candidate(args)
    try:
        result = smoke.entry(["--docker-context", args.docker_context, "--project", args.project,
                            "--evidence", str(args.evidence)], retain_legacy=False,
                           candidate_provider=candidate.prepare, exercise_extension=candidate.exercise,
                           before_cleanup=candidate.cleanup)
    finally:
        if candidate.owns_image:
            try:
                candidate.final_image_cleanup()
            except Exception as error:
                smoke.REPORT.update(passed=False, cleanup_verified=False,
                                    candidate_image_cleanup="failed", phase="failed")
                smoke.persist()
                raise smoke.Failure("owned candidate image cleanup failed") from error
            smoke.REPORT["candidate_image_cleanup"] = "removed"
            smoke.persist()
    return result


if __name__ == "__main__":
    raise SystemExit(main())
