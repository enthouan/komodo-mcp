"""Anonymous OCI graph inspection for the Docker Hub mirror."""

import hashlib
import json
import subprocess
from urllib import error, parse, request

from container_release import INDEX, MANIFEST, MAX_LAYER, REPOSITORY, TARGETS, check, describe, digest, verify_config

SOURCE = "ghcr.io/" + REPOSITORY
DESTINATION = "docker.io/" + REPOSITORY
MAX_MANIFEST = 4 * 1024 * 1024


class Missing(Exception):
    """The registry explicitly reported a missing manifest or blob."""


class Redirect(request.HTTPRedirectHandler):
    def redirect_request(self, req, fp, code, msg, headers, newurl):
        check(parse.urlsplit(newurl).scheme == "https", "insecure registry redirect")
        redirected = super().redirect_request(req, fp, code, msg, headers, newurl)
        if redirected:
            redirected.remove_header("Authorization")
        return redirected


def repository_public():
    url = "https://hub.docker.com/v2/namespaces/enthouan/repositories/komodo-mcp"
    with request.urlopen(url, timeout=30) as response:
        item = json.load(response)
    check(item["namespace"] == "enthouan" and item["name"] == "komodo-mcp" and item["is_private"] is False,
          "confirm the public Docker Hub repository before mirroring")


class Reader:
    def __init__(self, image, anonymous_env):
        self.image = image
        self.env = anonymous_env
        self.opener = request.build_opener(Redirect())
        if image == SOURCE:
            self.host, self.service, self.auth = "ghcr.io", "ghcr.io", "https://ghcr.io/token"
        elif image == DESTINATION:
            self.host, self.service, self.auth = "registry-1.docker.io", "registry.docker.io", "https://auth.docker.io/token"
        else:
            raise ValueError("unexpected mirror registry")
        self.token = None

    def open(self, kind, reference):
        url = f"https://{self.host}/v2/{REPOSITORY}/{kind}/{reference}"
        for attempt in range(2):
            if not self.token:
                params = parse.urlencode({"service": self.service, "scope": f"repository:{REPOSITORY}:pull"})
                with self.opener.open(self.auth + "?" + params, timeout=30) as response:
                    self.token = json.load(response)["token"]
            req = request.Request(url, headers={"Accept": f"{INDEX}, {MANIFEST}" if kind == "manifests" else "*/*"})
            req.add_unredirected_header("Authorization", "Bearer " + self.token)
            try:
                return self.opener.open(req, timeout=90)
            except error.HTTPError as failure:
                with failure:
                    if failure.code == 401 and attempt == 0:
                        self.token = None
                        continue
                    if failure.code == 404 and failure.geturl() == url:
                        body = json.loads(failure.read(8192)).get("errors", [])
                        absent = {"MANIFEST_UNKNOWN", "NAME_UNKNOWN"} if kind == "manifests" else {"BLOB_UNKNOWN"}
                        if body and all(e.get("code") in absent for e in body):
                            raise Missing(f"{self.image} is missing {kind}/{reference}") from None
                    raise RuntimeError(f"{self.image} {kind}/{reference}: HTTP {failure.code}") from None

    def manifest(self, ref):
        with self.open("manifests", ref) as response:
            raw = response.read(MAX_MANIFEST + 1)
            check(len(raw) <= MAX_MANIFEST and response.headers.get("Docker-Content-Digest") == describe(raw),
                  "manifest content digest or size mismatch")
        return raw

    def blob(self, desc, capture):
        identifier, size = descriptor(desc, MAX_LAYER)
        buf = bytearray()
        hashed = hashlib.sha256()
        length = 0
        with self.open("blobs", identifier) as response:
            while chunk := response.read(256 * 1024):
                length += len(chunk)
                check(length <= size, "oversized registry blob")
                hashed.update(chunk)
                if capture:
                    check(length <= MAX_MANIFEST, "oversized image config")
                    buf.extend(chunk)
        check(identifier == "sha256:" + hashed.hexdigest() and size == length, "blob identity mismatch")
        return bytes(buf)

    def referrers(self, identifier):
        result = subprocess.run(["regctl", "artifact", "list", self.image + "@" + identifier,
                                 "--format", "{{json .Descriptors}}"], check=True,
                                capture_output=True, env=self.env, timeout=120)
        found = json.loads(result.stdout)
        check(isinstance(found, list), "invalid referrers response")
        return found


def descriptor(item, limit):
    check(digest(item["digest"]) and type(item["size"]) is int and 0 <= item["size"] <= limit,
          "invalid OCI descriptor")
    return item["digest"], item["size"]


def tag_digest(reader, tag):
    try:
        return describe(reader.manifest(tag))
    except Missing:
        return None


def graph(reader, identifier, version, revision):
    """Verify the index, its descriptor closure and the attached OCI referrers, including their blobs."""
    roots = reader.manifest(identifier)
    check(describe(roots) == identifier, "index digest mismatch")
    root = json.loads(roots)
    check(root["mediaType"] == INDEX and root["schemaVersion"] == 2, "expected an OCI index")
    platforms = {}
    for item in root["manifests"]:
        platform = item.get("platform", {})
        name = f"{platform.get('os')}/{platform.get('architecture')}"
        if name in TARGETS:
            check(name not in platforms, "duplicate Linux platform")
            platforms[name] = item["digest"]
    check(set(platforms) == set(TARGETS) and len(set(platforms.values())) == len(TARGETS), "wrong Linux platforms")

    seen = {}
    blobs = set()
    refs = {}
    by_digest = {value: key for key, value in platforms.items()}

    def walk(raw, key):
        if key in seen:
            return
        check(len(seen) < 512, "oversized mirror graph")
        seen[key] = len(raw)
        node = json.loads(raw)
        children = []
        if node["mediaType"] == INDEX:
            children.extend(node["manifests"])
        else:
            check(node["mediaType"] == MANIFEST and node["schemaVersion"] == 2, "unsupported child manifest")
            for pos, desc in enumerate([node["config"], *node["layers"]]):
                blob_key = descriptor(desc, MAX_LAYER)
                capture = pos == 0 and key in by_digest
                if capture or blob_key not in blobs:
                    data = reader.blob(desc, capture)
                    if capture:
                        verify_config(json.loads(data), version, revision, by_digest[key])
                    blobs.add(blob_key)
        attachments = reader.referrers(key)
        refs[key] = sorted(item["digest"] for item in attachments)
        for desc in children + attachments:
            child, size = descriptor(desc, MAX_MANIFEST)
            if child in seen:
                check(seen[child] == size, "manifest size mismatch")
                continue
            data = reader.manifest(child)
            check(describe(data) == child and len(data) == size, "child manifest identity mismatch")
            walk(data, child)

    walk(roots, identifier)
    return {"index_digest": identifier, "platforms": platforms, "referrers": refs, "blob_count": len(blobs)}
