"""Native read-only MCP workflows for Kalaghni/komodo-mcp 0.1.0."""
import json
from core import digest
from stdio import project


class Reader:
    """Count the comparator's real JSON-RPC bytes, including its full text payload."""

    def __init__(self, client):
        self.client = client
        self.calls = 0
        self.response_bytes = 0

    async def read(self, operation, **params):
        # komodo_list does not accept page/limit. The pinned implementation's
        # public read tool allows callers to follow Core pagination explicitly.
        result, size = await self.client.request("tools/call", {
            "name": "komodo_read", "arguments": {"request": operation, "params": params}})
        self.calls += 1
        self.response_bytes += size
        content = result.get("content")
        if (result.get("isError", False) is not False or not isinstance(content, list)
                or len(content) != 1 or content[0].get("type") != "text"):
            raise ValueError(f"Rust reference failed to return one text value for {operation}")
        try:
            return json.loads(content[0]["text"])
        except (KeyError, ValueError) as error:
            raise ValueError(f"Rust reference returned truncated or invalid JSON for {operation}") from error

    async def inventory(self, page_size, expected_count):
        items = []
        for page in range(expected_count // page_size + 2):
            rows = await self.read("ListStacks", page=page, limit=page_size, sort_by="Name", sort_desc=False)
            if not isinstance(rows, list) or len(rows) > page_size:
                raise ValueError("invalid Rust reference stack page")
            items.extend(project([{"id": row["id"], "name": row["name"], "state": row["info"]["state"].lower()}
                                  for row in rows]))
            if len(rows) < page_size:
                return items
        raise ValueError("Rust reference stack pagination exceeded its bound")

    async def diagnose(self, page_size, expected_count):
        stacks = await self.inventory(page_size, expected_count)
        affected = [s for s in stacks if s["state"] == "unhealthy"]
        if len(affected) != 1 or len(stacks) != expected_count or len({s["id"] for s in stacks}) != len(stacks):
            raise ValueError("Rust reference found incomplete fleet or ambiguous incident")
        stack = affected[0]
        rows = await self.read("ListAllStackServices", stacks=[stack["id"]], page=0, limit=101)
        services = [row["service"] for row in rows
                    if row["stack_id"] == stack["id"] and row["state"].lower() == "unhealthy"]
        if len(services) != 1:
            raise ValueError("Rust reference found ambiguous unhealthy service")
        history = await self.read("ListUpdates", query={"target.type": "Stack", "target.id": stack["id"]}, page=0)
        failed = [row["id"] for row in history["updates"]
                  if row["success"] is False and row["status"] == "Complete"
                  and row["target"] == {"type": "Stack", "id": stack["id"]}]
        if len(failed) != 1:
            raise ValueError("Rust reference found ambiguous failed update")
        update = await self.read("GetUpdate", id=failed[0])
        stages = [row["stage"] for row in update["logs"] if row["success"] is False]
        if (len(stages) != 1 or update["_id"]["$oid"] != failed[0] or update["success"] is not False
                or update["status"] != "Complete" or update["target"] != {"type": "Stack", "id": stack["id"]}):
            raise ValueError("Rust reference update detail failed stage or identity checks")
        log = await self.read("GetStackLog", stack=stack["id"], services=services, tail=40, timestamps=False)
        excerpts = [line for channel in ("stdout", "stderr") for line in log.get(channel, "").splitlines()
                    if "ERROR:" in line]
        if len(excerpts) != 1:
            raise ValueError("Rust reference found ambiguous error excerpt")
        return {"stack_id": stack["id"], "stack_name": stack["name"], "service": services[0],
                "update_id": failed[0], "failed_stage": stages[0], "log_excerpt": excerpts[0],
                "visible_stacks": len(stacks), "discovery_sha256": digest(sorted(stacks, key=lambda s: s["id"]))}
