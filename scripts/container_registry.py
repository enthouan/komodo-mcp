"""Read GHCR identities and promote one content-verified OCI candidate for release.py."""

import base64
import json
import os
import subprocess
import tarfile
from urllib import error, parse, request

from container_release import (
    CONFIG,
    INDEX,
    MANIFEST,
    MAX_LAYER,
    REPOSITORY,
    check,
    content,
    describe,
    digest,
    tar_members,
    validate_platforms,
)

HOST = "https://ghcr.io"
BASE = HOST + "/v2/" + REPOSITORY
LIMIT = 2**20


class NoRedirect(request.HTTPRedirectHandler):
    def redirect_request(self, *args):
        return None


def exchange(method, url, *, authorization, body=None, headers=None):
    """Never forward credentials or signed upload locations outside the registry."""
    destination = parse.urlsplit(url)
    check(
        destination.scheme == "https" and destination.netloc == "ghcr.io" and not destination.fragment,
        "unexpected GHCR destination",
    )
    fields = {"Authorization": authorization} if authorization else {}
    fields.update(headers or {})
    req = request.Request(url, method=method, headers=fields, data=body)
    opener = request.build_opener(request.ProxyHandler({}), NoRedirect())
    try:
        try:
            response = opener.open(req, timeout=90)
        except error.HTTPError as exc:
            response = exc
        with response:
            data = response.read(LIMIT + 1)
            check(len(data) <= LIMIT, "oversized GHCR response")
            return response.status, response.headers, data
    except (OSError, error.URLError):
        raise RuntimeError("GHCR request failed; read the remote state before retrying") from None


def credentials():
    user = os.environ.get("GHCR_USERNAME")
    token = os.environ.get("GHCR_TOKEN")
    if user is None and token is None:
        try:

            def gh(*args):
                return subprocess.check_output(
                    ["gh", *args], text=True, stderr=subprocess.DEVNULL, timeout=20
                ).strip()

            user = gh("api", "--hostname", "github.com", "user", "--jq", ".login")
            token = gh("auth", "token", "--hostname", "github.com")
        except (OSError, subprocess.SubprocessError):
            raise RuntimeError("GHCR credentials are unavailable") from None
    check(user and token and ":" not in user, "set both GHCR_USERNAME and GHCR_TOKEN")
    return "Basic " + base64.b64encode(f"{user}:{token}".encode()).decode()


def access(*, anonymous=False, write=False):
    scope = "pull,push" if write else "pull"
    query = parse.urlencode({"service": "ghcr.io", "scope": f"repository:{REPOSITORY}:{scope}"})
    status, _, data = exchange(
        "GET", HOST + "/token?" + query, authorization=None if anonymous else credentials()
    )
    check(status == 200, "GHCR did not authorize the requested registry access")
    value = json.loads(data).get("token")
    check(
        isinstance(value, str) and value and value.isascii() and not any(c.isspace() for c in value),
        "GHCR returned an invalid access token",
    )
    return "Bearer " + value


def absent(body):
    try:
        errors = json.loads(body)["errors"]
        return (
            isinstance(errors, list)
            and len(errors) > 0
            and all(entry["code"] in {"MANIFEST_UNKNOWN", "NAME_UNKNOWN"} for entry in errors)
        )
    except (KeyError, ValueError, TypeError):
        return False


def read_manifest(reference, auth, media, expected, *, allow_absent=False):
    status, headers, raw = exchange(
        "GET", BASE + "/manifests/" + reference, authorization=auth, headers={"Accept": media}
    )
    if allow_absent and status == 404 and absent(raw):
        return None
    check(status == 200, "GHCR manifest unavailable or visibility inconclusive")
    check(
        describe(raw) == expected and headers.get("Docker-Content-Digest", expected) == expected,
        "GHCR manifest conflicts with the release candidate",
    )
    return json.loads(raw)


def state(receipt, auth, *, allow_absent):
    index = read_manifest(
        receipt["image"].rsplit(":", 1)[1], auth, INDEX, receipt["index_digest"], allow_absent=allow_absent
    )
    if index is None:
        return None
    verified_blobs = set()
    for platform, desc in validate_platforms(index).items():
        expected = receipt["platforms"][platform]
        check(desc["digest"] == expected["manifest_digest"], "GHCR platform manifest differs")
        child = read_manifest(desc["digest"], auth, MANIFEST, expected["manifest_digest"])
        check(
            child.get("mediaType") == MANIFEST
            and child["config"]["mediaType"] == CONFIG
            and child["config"]["digest"] == expected["config_digest"],
            "GHCR platform config differs",
        )
        check(
            isinstance(child["layers"], list) and 0 < len(child["layers"]) <= 32, "invalid GHCR layer count"
        )
        for entry in [child["config"], *child["layers"]]:
            identifier = entry["digest"]
            check(
                digest(identifier) and type(entry["size"]) is int and 0 < entry["size"] <= MAX_LAYER,
                "invalid GHCR blob descriptor",
            )
            identity = (identifier, entry["size"])
            if identity in verified_blobs:
                continue
            status, headers, _ = exchange("HEAD", BASE + "/blobs/" + identifier, authorization=auth)
            check(
                status == 200
                and headers.get("Docker-Content-Digest") == identifier
                and headers.get("Content-Length") == str(entry["size"]),
                "GHCR image blob unavailable or has the wrong digest or length",
            )
            verified_blobs.add(identity)
    return {key: receipt[key] for key in ("image", "index_digest", "platforms")}


def remote_state(receipt, *, anonymous=False):
    return state(receipt, access(anonymous=anonymous), allow_absent=not anonymous)


def upload_blob(auth, identifier, payload):
    status, headers, _ = exchange("HEAD", BASE + "/blobs/" + identifier, authorization=auth)
    if status == 200:
        check(headers.get("Docker-Content-Digest", identifier) == identifier, "GHCR blob digest differs")
        return
    check(status == 404, "GHCR blob presence is inconclusive")
    start = BASE + "/blobs/uploads/"
    status, headers, _ = exchange("POST", start, authorization=auth, body=b"")
    check(status == 202 and headers.get("Location"), "GHCR did not open a blob upload")
    location = parse.urlsplit(parse.urljoin(start, headers["Location"]))
    # GHCR opens a session at "uploads/" and can return its continuation under "upload/".
    blob_path = parse.urlsplit(BASE).path + "/blobs/"
    check(
        location.scheme == "https"
        and location.netloc == "ghcr.io"
        and location.path.startswith((blob_path + "upload/", blob_path + "uploads/"))
        and not location.fragment,
        "unexpected GHCR upload location",
    )
    query = parse.parse_qsl(location.query, keep_blank_values=True)
    check(all(key != "digest" for key, _ in query), "unexpected GHCR upload digest")
    # Upload locations are opaque. Keep the registry's existing query bytes intact.
    suffix = "&" if location.query else ""
    url = location._replace(
        query=location.query + suffix + parse.urlencode({"digest": identifier})
    ).geturl()
    status, headers, _ = exchange(
        "PUT", url, authorization=auth, body=payload, headers={"Content-Type": "application/octet-stream"}
    )
    check(
        status == 201 and headers.get("Docker-Content-Digest", identifier) == identifier,
        "GHCR did not confirm the uploaded blob",
    )


def put_manifest(reference, auth, media, raw):
    status, headers, _ = exchange(
        "PUT",
        BASE + "/manifests/" + reference,
        authorization=auth,
        headers={"Content-Type": media},
        body=raw,
    )
    check(
        status == 201 and headers.get("Docker-Content-Digest", describe(raw)) == describe(raw),
        "GHCR did not confirm the manifest write",
    )


def stage(candidate, receipt, auth):
    """Stage blobs and platform manifests exclusively by digest. Return the verified raw index."""
    with tarfile.open(candidate, "r:") as archive:
        members = tar_members(archive)

        def blob(identifier, limit):
            data = content(archive, members, "blobs/sha256/" + identifier[7:], limit)
            check(describe(data) == identifier, "candidate blob changed before staging")
            return data

        raw_index = blob(receipt["index_digest"], LIMIT)
        uploaded = set()
        for platform, desc in validate_platforms(json.loads(raw_index)).items():
            check(
                desc["digest"] == receipt["platforms"][platform]["manifest_digest"],
                "candidate platform changed before staging",
            )
            raw = blob(desc["digest"], LIMIT)
            manifest = json.loads(raw)
            for entry in [manifest["config"], *manifest["layers"]]:
                identifier = entry["digest"]
                if identifier not in uploaded:
                    upload_blob(auth, identifier, blob(identifier, MAX_LAYER))
                    uploaded.add(identifier)
            put_manifest(desc["digest"], auth, MANIFEST, raw)
        return raw_index


def promote(candidate, receipt):
    auth = access(write=True)
    existing = state(receipt, auth, allow_absent=True)
    if existing is not None:
        return existing
    raw_index = stage(candidate, receipt, auth)
    existing = state(receipt, auth, allow_absent=True)
    if existing is not None:
        return existing
    # Registry tags have no cross-publisher compare-and-set. Serialize release publishers.
    try:
        put_manifest(receipt["image"].rsplit(":", 1)[1], auth, INDEX, raw_index)
    except (OSError, RuntimeError, ValueError):
        outcome = state(receipt, auth, allow_absent=True)
        if outcome is not None:
            return outcome
        raise RuntimeError(
            "GHCR version tag write was not confirmed; recover from release evidence"
        ) from None
    return state(receipt, auth, allow_absent=False)
