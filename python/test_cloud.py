"""The cloud client's Python bindings, driven with canned responses (the
protocol itself is checked against rmapi-js by tests/reference/cloud-check.mjs)."""

import hashlib
import json

import pytest

from remarkable_annotations import CloudSession, cloud_auth, cloud_register

# payload {"device-id": "dev"}
TOKEN = "e30.eyJkZXZpY2UtaWQiOiJkZXYifQ.sig"


def drive(op, responses, batches=None):
    """Runs `op`, answering each request with `responses(request)`; records
    the size of each batch in `batches`."""

    def perform(item):
        return None if item["type"] == "sleep" else responses(item)

    step = op.start()
    while step["type"] in ("request", "sleep", "batch"):
        if step["type"] == "batch":
            if batches is not None:
                batches.append(len(step["items"]))
            step = op.resume_batch([perform(i) for i in step["items"]])
        else:
            step = op.resume(perform(step))
    return step


def test_register_and_auth():
    step = cloud_register(
        "ABCDEFGH", "browser-chrome", "uuid-1", "https://auth.test"
    ).start()
    assert step["type"] == "request"
    assert step["url"] == "https://auth.test/token/json/2/device/new"
    assert json.loads(step["body"]) == {
        "code": "ABCDEFGH",
        "deviceDesc": "browser-chrome",
        "deviceID": "uuid-1",
    }
    done = drive(
        cloud_auth("device-token", "https://auth.test"),
        lambda r: {"status": 200, "body": TOKEN.encode()},
    )
    assert done == {"type": "done", "text": TOKEN}
    failed = drive(cloud_auth("bad"), lambda r: {"status": 401, "body": b"no"})
    assert failed["type"] == "error"
    assert json.loads(failed["error"])["kind"] == "auth"


def test_list_refs():
    index = b"4\n0:.:1:10\n" + b"a" * 64 + b":0:doc-1:2:10\n"
    root_hash = hashlib.sha256(index).hexdigest()
    session = CloudSession(TOKEN, json.dumps({"rawHost": "https://raw.test"}))
    assert session.device_id == "dev"

    def responses(r):
        if r["url"].endswith("/sync/v4/root"):
            body = {"hash": root_hash, "generation": 3, "schemaVersion": 4}
            return {"status": 200, "body": json.dumps(body).encode()}
        assert dict(r["headers"])["rm-filename"] == "root.docSchema"
        return {"status": 200, "body": index}

    done = drive(session.list_refs(), responses)
    assert json.loads(done["json"]) == [{"id": "doc-1", "hash": "a" * 64}]


def test_list_items_in_batches():
    files = {}

    def put(b):
        h = hashlib.sha256(b).hexdigest()
        files[h] = b
        return h

    lines = []
    for i in range(2):
        doc = f"doc-{i}"
        content = put(b'{"tags":[]}')
        meta = put(
            json.dumps(
                {
                    "visibleName": f"F{i}",
                    "type": "CollectionType",
                    "parent": "",
                    "pinned": False,
                }
            ).encode()
        )
        index = put(
            f"4\n0:{doc}:2:0\n{content}:0:{doc}.content:0:0\n"
            f"{meta}:0:{doc}.metadata:0:0\n".encode()
        )
        lines.append(f"{index}:0:{doc}:2:0\n")
    root = put(("4\n0:.:2:0\n" + "".join(lines)).encode())
    session = CloudSession(TOKEN, json.dumps({"rawHost": "https://raw.test"}))

    def responses(r):
        if r["url"].endswith("/sync/v4/root"):
            body = {"hash": root, "generation": 3, "schemaVersion": 4}
            return {"status": 200, "body": json.dumps(body).encode()}
        return {"status": 200, "body": files[r["url"].rsplit("/", 1)[1]]}

    batches = []
    done = drive(session.list_items(True), responses, batches)
    assert done["type"] == "done", done
    items = json.loads(done["json"])
    assert [i["visibleName"] for i in items] == ["F0", "F1"]
    # the two indexes, then the two metadata and contents, at once
    assert batches == [2, 4]
    with pytest.raises(TypeError):
        session.list_items(True).resume_batch("not a list")


def test_network_errors_are_retried():
    session = CloudSession(TOKEN, json.dumps({"rawHost": "https://raw.test"}))
    done = drive(session.list_refs(True), lambda r: {"network_error": "down"})
    assert done["type"] == "error"
    assert json.loads(done["error"])["kind"] == "network"


def test_bad_token():
    with pytest.raises(ValueError):
        CloudSession("not-a-jwt")
