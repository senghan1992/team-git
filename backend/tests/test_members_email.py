"""Tests for email-based project member invites and auto-join on device registration."""
import pytest
from app.db import Session
from app.models import ProjectMember
from tests.conftest import make_device


def test_invite_by_email_returns_pending(client):
    """Invite alice@example.com → assert pending=True."""
    db = Session()
    _, token_owner = make_device(db, "owner")
    db.commit()
    db.close()

    # Create project
    resp = client.post(
        "/projects",
        json={"display_name": "Test Project"},
        headers={"Authorization": f"Bearer {token_owner}"},
    )
    assert resp.status_code == 200
    project_id = resp.json()["id"]

    # Invite by email
    resp = client.post(
        f"/projects/{project_id}/members/email",
        json={"email": "alice@example.com", "name": "Alice", "role": "member"},
        headers={"Authorization": f"Bearer {token_owner}"},
    )
    assert resp.status_code == 200
    data = resp.json()
    assert data["pending"] is True
    assert data["email"] == "alice@example.com"
    assert data["role"] == "member"


def test_device_registers_with_email_auto_joined(client):
    """Register device with matching email → auto-added to project as member."""
    db = Session()
    _, token_owner = make_device(db, "owner")
    db.commit()
    db.close()

    # Owner creates project
    resp = client.post(
        "/projects",
        json={"display_name": "Auto Join Project"},
        headers={"Authorization": f"Bearer {token_owner}"},
    )
    assert resp.status_code == 200
    project_id = resp.json()["id"]

    # Owner invites alice@example.com
    client.post(
        f"/projects/{project_id}/members/email",
        json={"email": "alice@example.com", "role": "member"},
        headers={"Authorization": f"Bearer {token_owner}"},
    )

    # Alice registers device WITH her email → auto-joins project
    resp = client.post(
        "/devices/register",
        json={"name": "Alice's Laptop", "email": "alice@example.com"},
    )
    assert resp.status_code == 200
    alice_device_id = resp.json()["id"]

    # Verify alice's device is in project_members
    db = Session()
    member = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == project_id,
            ProjectMember.device_id == alice_device_id,
        )
        .first()
    )
    db.close()
    assert member is not None, "alice should be auto-added as a project member"
    assert member.role == "member"


def test_remove_email_invite(client):
    """Delete email invite → gone from list."""
    db = Session()
    _, token_owner = make_device(db, "owner")
    db.commit()
    db.close()

    # Owner creates project
    resp = client.post(
        "/projects",
        json={"display_name": "Remove Test"},
        headers={"Authorization": f"Bearer {token_owner}"},
    )
    assert resp.status_code == 200
    project_id = resp.json()["id"]

    # Invite bob@example.com
    client.post(
        f"/projects/{project_id}/members/email",
        json={"email": "bob@example.com", "role": "member"},
        headers={"Authorization": f"Bearer {token_owner}"},
    )

    # Remove invite
    resp = client.delete(
        f"/projects/{project_id}/members/email/bob@example.com",
        headers={"Authorization": f"Bearer {token_owner}"},
    )
    assert resp.status_code == 200

    # Verify gone
    resp = client.get(
        f"/projects/{project_id}/members/email",
        headers={"Authorization": f"Bearer {token_owner}"},
    )
    assert resp.status_code == 200
    members = resp.json()["members"]
    emails = [m["email"] for m in members if m.get("email")]
    assert "bob@example.com" not in emails
