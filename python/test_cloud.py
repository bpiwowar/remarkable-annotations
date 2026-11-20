"""The cloud client's Python bindings, driven with canned responses (the
protocol itself is checked against rmapi-js by tests/reference/cloud-check.mjs)."""

import hashlib
import json

import pytest

from remarkable_annotations import CloudSession, cloud_auth, cloud_register

# payload {"device-id": "dev"}
TOKEN = "e30.eyJkZXZpY2UtaWQiOiJkZXYifQ.sig"


def drive(op, responses):
    """Runs `op`, answering each request with `responses(request)`."""
    step = op.start()
    while step["type"] in ("request", "sleep"):
        if step["type"] == "request":
            step = op.resume(responses(step))
        else:
            step = op.resume(None)
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


def test_network_errors_are_retried():
    session = CloudSession(TOKEN, json.dumps({"rawHost": "https://raw.test"}))
    done = drive(session.list_refs(True), lambda r: {"network_error": "down"})
    assert done["type"] == "error"
    assert json.loads(done["error"])["kind"] == "network"


def test_bad_token():
    with pytest.raises(ValueError):
        CloudSession("not-a-jwt")
