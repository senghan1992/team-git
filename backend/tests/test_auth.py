"""Tests for authentication requirements."""
import pytest
from app.auth import generate_token, hash_token
from app.db import Session
from app.models import Device
import uuid


def _make_device() -> tuple[str, str]:
    token = generate_token()
    device_id = str(uuid.uuid4().hex)
    db = Session()
    db.add(Device(
        id=device_id,
        user_id=str(uuid.uuid4().hex),
        name="test",
        token_hash=hash_token(token),
    ))
    db.commit()
    db.close()
    return device_id, token


def test_missing_token_rejected(client):
    """Any endpoint requiring auth returns 401 when no token is provided."""
    protected = [
        ("/projects", "get"),
        ("/devices/me", "get"),
        ("/events/poll", "post"),
    ]
    for url, method in protected:
        resp = getattr(client, method)(url)
        assert resp.status_code == 401, f"{method.upper()} {url} should reject missing token"


def test_unknown_token_rejected(client):
    """A token that was never registered returns 401."""
    resp = client.get(
        "/projects",
        headers={"Authorization": "Bearer invalid.token.here"},
    )
    assert resp.status_code == 401


def test_bearer_format_required(client):
    """Malformed Authorization header returns 401."""
    resp = client.get("/projects", headers={"Authorization": "NotBearer sometoken"})
    assert resp.status_code == 401

    resp = client.get("/projects", headers={"Authorization": "Basic sometoken"})
    assert resp.status_code == 401
