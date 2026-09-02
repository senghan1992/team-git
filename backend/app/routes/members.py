"""Project membership management endpoints."""
from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.orm import Session

from app.deps import get_db, get_device
from app.models import Device, Project, ProjectMember, ProjectMemberEmail
from app.schemas import (
    MemberInfo,
    MemberListResponse,
    MemberAddByEmailRequest,
    MemberAddByEmailResponse,
)

router = APIRouter()


def _require_member(project_id: str, device: Device, db: Session) -> ProjectMember:
    """Raise 403 if caller is not a member."""
    caller = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == project_id,
            ProjectMember.device_id == device.id,
        )
        .first()
    )
    if not caller:
        raise HTTPException(status_code=403, detail="Not a member of this project")
    return caller


@router.get("/{project_id}/members", response_model=MemberListResponse)
def list_members(
    project_id: str,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """List all members of a project (must be a member to list)."""
    _require_member(project_id, device, db)

    rows = (
        db.query(ProjectMember, Device)
        .join(Device, ProjectMember.device_id == Device.id)
        .filter(ProjectMember.project_id == project_id)
        .all()
    )
    members = [
        MemberInfo(
            device_id=pm.device_id,
            email=None,
            name=d.name,
            role=pm.role,
            joined_at=pm.joined_at,
        )
        for pm, d in rows
    ]
    return MemberListResponse(members=members)


@router.delete("/{project_id}/members/{member_device_id}")
def remove_member(
    project_id: str,
    member_device_id: str,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """
    Remove a member from a project.

    Only owners can remove others. Members can remove themselves.
    """
    caller = _require_member(project_id, device, db)

    target = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == project_id,
            ProjectMember.device_id == member_device_id,
        )
        .first()
    )
    if not target:
        raise HTTPException(status_code=404, detail="Member not found")

    # Self-removal always allowed; removing others requires owner role
    if member_device_id != device.id and caller.role != "owner":
        raise HTTPException(status_code=403, detail="Only owners can remove other members")

    # Owners cannot be removed (except by leaving the project entirely — deferred)
    if target.role == "owner" and member_device_id != device.id:
        raise HTTPException(status_code=403, detail="Cannot remove the owner")

    db.delete(target)
    db.commit()
    return {"ok": True}


# ── Email-based invites ────────────────────────────────────────────────────────


@router.post("/{project_id}/members/email", response_model=MemberAddByEmailResponse)
def add_member_by_email(
    project_id: str,
    body: MemberAddByEmailRequest,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """
    Invite someone by email. They are added as pending until they register a device.
    Only owners and maintainers may invite.
    """
    caller = _require_member(project_id, device, db)
    if caller.role not in ("owner", "maintainer"):
        raise HTTPException(status_code=403, detail="Only owners/maintainers can invite")

    # Upsert the pending email invite.
    existing = (
        db.query(ProjectMemberEmail)
        .filter(
            ProjectMemberEmail.project_id == project_id,
            ProjectMemberEmail.email == body.email.lower(),
        )
        .first()
    )
    if existing:
        existing.name = body.name
        existing.role = body.role
    else:
        invite = ProjectMemberEmail(
            project_id=project_id,
            email=body.email.lower(),
            name=body.name,
            role=body.role,
        )
        db.add(invite)

    db.commit()
    return MemberAddByEmailResponse(
        device_id=None,
        email=body.email.lower(),
        role=body.role,
        pending=True,
    )


@router.get("/{project_id}/members/email", response_model=MemberListResponse)
def list_members_by_email(
    project_id: str,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """
    List all project members and pending email invites, merged into one list.
    """
    _require_member(project_id, device, db)

    # Active device members.
    rows = (
        db.query(ProjectMember, Device)
        .join(Device, ProjectMember.device_id == Device.id)
        .filter(ProjectMember.project_id == project_id)
        .all()
    )
    members = [
        MemberInfo(
            device_id=pm.device_id,
            email=None,
            name=d.name,
            role=pm.role,
            joined_at=pm.joined_at,
        )
        for pm, d in rows
    ]

    # Pending email invites.
    invites = (
        db.query(ProjectMemberEmail)
        .filter(ProjectMemberEmail.project_id == project_id)
        .all()
    )
    for inv in invites:
        members.append(
            MemberInfo(
                device_id=None,
                email=inv.email,
                name=inv.name,
                role=inv.role,
                joined_at=inv.invited_at,
            )
        )

    return MemberListResponse(members=members)


@router.delete("/{project_id}/members/email/{email}")
def remove_email_invite(
    project_id: str,
    email: str,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """
    Remove a pending email invite. Only owners can remove invites of others.
    """
    caller = _require_member(project_id, device, db)

    invite = (
        db.query(ProjectMemberEmail)
        .filter(
            ProjectMemberEmail.project_id == project_id,
            ProjectMemberEmail.email == email.lower(),
        )
        .first()
    )
    if not invite:
        raise HTTPException(status_code=404, detail="Email invite not found")

    if caller.role != "owner":
        raise HTTPException(status_code=403, detail="Only owners can remove email invites")

    db.delete(invite)
    db.commit()
    return {"ok": True}
