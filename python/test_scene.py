"""The Python bindings: fixtures round-trip, and parse like rmapi-js."""

import json
from pathlib import Path

import pytest

from remarkable_annotations import Scene

FIXTURES = sorted((Path(__file__).parent.parent / "tests/fixtures/rm").rglob("*.rm"))


@pytest.mark.parametrize("path", FIXTURES, ids=lambda p: p.name)
def test_round_trip(path: Path):
    data = path.read_bytes()
    assert Scene(data).serialize() == data


@pytest.mark.parametrize("path", FIXTURES, ids=lambda p: p.name)
def test_blocks(path: Path):
    reference = json.loads(path.with_suffix(".rmapi.json").read_text())
    blocks = json.loads(Scene(path.read_bytes()).blocks_json())
    assert len(blocks) == len(reference["blocks"])
    assert [b["type"] for b in blocks] == [b["type"] for b in reference["blocks"]]


def test_not_v6():
    with pytest.raises(ValueError):
        Scene(b"not a reMarkable file")
