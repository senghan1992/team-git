"""Tests for project create / join roundtrip."""
import pytest
from tests.conftest import make_device


def test_create_then_join_roundtrip(client, auth_headers):
    """Owner creates a project; a second device joins using the join code."""
    from app.db import Session

    resp = client.post("/projects", json={"display_name": "My Team"}, headers=auth_headers)
    assert resp.status_code == 200
    data = resp.json()
    assert data["display_name"] == "My Team"
    assert data["role"] == "owner"
    join_code = data["join_code"]

    db = Session()
    _, token_b = make_device(db)
    db.close()

    resp = client.post(
        "/projects/join",
        json={"join_code": join_code},
        headers={"Authorization": f"Bearer {token_b}"},
    )
    assert resp.status_code == 200
    assert resp.json()["role"] == "member"
    assert resp.json()["id"] == data["id"]

    resp = client.get("/projects", headers=auth_headers)
    assert resp.status_code == 200
    assert len(resp.json()["projects"]) == 1

    resp = client.get("/projects", headers={"Authorization": f"Bearer {token_b}"})
    assert resp.status_code == 200
    assert len(resp.json()["projects"]) == 1


def test_join_invalid_code_rejected(client, auth_headers):
    resp = client.post(
        "/projects/join",
        json={"join_code": "XXXX-XXXX"},
        headers=auth_headers,
    )
    assert resp.status_code == 404


def test_join_duplicate_member_rejected(client, auth_headers):
    resp = client.post("/projects", json={"display_name": "Solo"}, headers=auth_headers)
    assert resp.status_code == 200
    join_code = resp.json()["join_code"]

    resp = client.post(
        "/projects/join",
        json={"join_code": join_code},
        headers=auth_headers,
    )
    assert resp.status_code == 409
