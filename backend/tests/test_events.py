"""Tests for event fan-out, polling, and acknowledgement."""
import json
import pytest
from app.db import Session
from app.models import EventDelivery
from tests.conftest import make_device


def test_fanout_creates_pending_delivery_records(client):
    """Event creation persists an EventDelivery row for each subscriber (sender excluded)."""
    db = Session()
    _, token_a = make_device(db, "device-a")
    _, token_b = make_device(db, "device-b")
    db.close()

    # A creates project
    resp = client.post(
        "/projects",
        json={"display_name": "Team"},
        headers={"Authorization": f"Bearer {token_a}"},
    )
    project_id = resp.json()["id"]
    join_code = resp.json()["join_code"]

    # B joins
    resp = client.post(
        "/projects/join",
        json={"join_code": join_code},
        headers={"Authorization": f"Bearer {token_b}"},
    )
    assert resp.status_code == 200

    # A fires an event
    payload = json.dumps({"author": "alice", "message": "fix bug", "sha": "abc123"})
    resp = client.post(
        "/events",
        json={
            "project_id": project_id,
            "event_kind": "main_push",
            "repo_name": "my-repo",
            "payload": payload,
        },
        headers={"Authorization": f"Bearer {token_a}"},
    )
    assert resp.status_code == 200
    event_id = resp.json()["id"]

    db = Session()
    deliveries = db.query(EventDelivery).filter(EventDelivery.event_id == event_id).all()
    db.close()

    # Only B (the non-sender subscriber) should have a delivery record
    assert len(deliveries) == 1


def test_event_retries_when_subscriber_offline_then_picks_up_on_next_poll(client):
    """Offline B gets the event on next poll via durable EventDelivery row."""
    db = Session()
    _, token_a = make_device(db, "device-a")
    _, token_b = make_device(db, "device-b")
    db.close()

    # A creates project
    resp = client.post(
        "/projects",
        json={"display_name": "Team"},
        headers={"Authorization": f"Bearer {token_a}"},
    )
    project_id = resp.json()["id"]
    join_code = resp.json()["join_code"]

    # B joins
    resp = client.post(
        "/projects/join",
        json={"join_code": join_code},
        headers={"Authorization": f"Bearer {token_b}"},
    )
    assert resp.status_code == 200

    # B is offline. A fires an event.
    payload = json.dumps({"author": "alice", "message": "hotfix", "sha": "def456"})
    resp = client.post(
        "/events",
        json={
            "project_id": project_id,
            "event_kind": "branch_push",
            "repo_name": "my-repo",
            "payload": payload,
        },
        headers={"Authorization": f"Bearer {token_a}"},
    )
    assert resp.status_code == 200
    event_id = resp.json()["id"]

    # B polls and receives the event
    resp = client.post(
        "/events/poll",
        json={"wait": 2},
        headers={"Authorization": f"Bearer {token_b}"},
    )
    assert resp.status_code == 200
    body = resp.json()
    assert body["event"] is not None
    assert body["event"]["id"] == event_id


def test_cross_project_delivery_rejected(client):
    """A device cannot send an event to a project it is not a member of."""
    db = Session()
    _, token_a = make_device(db, "device-a")
    _, token_b = make_device(db, "device-b")
    db.close()

    resp = client.post(
        "/projects",
        json={"display_name": "Project A"},
        headers={"Authorization": f"Bearer {token_a}"},
    )
    project_a_id = resp.json()["id"]

    resp = client.post(
        "/projects",
        json={"display_name": "Project B"},
        headers={"Authorization": f"Bearer {token_b}"},
    )
    project_b_id = resp.json()["id"]

    resp = client.post(
        "/events",
        json={
            "project_id": project_b_id,
            "event_kind": "main_push",
            "repo_name": "repo",
            "payload": "{}",
        },
        headers={"Authorization": f"Bearer {token_a}"},
    )
    assert resp.status_code == 403
