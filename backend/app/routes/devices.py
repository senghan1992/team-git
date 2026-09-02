"""Device registration and management endpoints."""
import uuid

from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.orm import Session

from app.auth import generate_token, hash_token
from app.deps import get_db, get_device
from app.models import Device, ProjectMember, ProjectMemberEmail
from app.schemas import (
    DeviceRegisterRequest,
    DeviceRegisterResponse,
    DeviceMeResponse,
    DeviceUpdatePollUrl,
)

router = APIRouter()


@router.post("/register", response_model=DeviceRegisterResponse)
def register_device(
    body: DeviceRegisterRequest,
    db: Session = Depends(get_db),
):
    """
    Register a new device and return a bearer token.

    The caller should store the returned token and send it as
    ``Authorization: Bearer <token>`` on every subsequent request.

    If `email` is provided and matches a pending ``ProjectMemberEmail`` row,
    the device is automatically added to that project as a member.
    The pending invite row is NOT deleted — it remains until explicitly removed.
    """
    user_id = str(uuid.uuid4().hex)

    device = Device(
        id=str(uuid.uuid4().hex),
        user_id=user_id,
        name=body.name,
        token_hash="",  # filled below
    )

    # Generate a new token for this device
    token = generate_token()
    device.token_hash = hash_token(token)

    db.add(device)

    # Auto-join projects for which this email has a pending invite.
    if body.email:
        for inv in db.query(ProjectMemberEmail).filter(
            ProjectMemberEmail.email == body.email.lower()
        ).all():
            member = ProjectMember(
                project_id=inv.project_id,
                device_id=device.id,
                role=inv.role,
            )
            db.add(member)
            # Invite row is NOT deleted — remains until explicitly removed by owner.

    db.commit()
    db.refresh(device)

    # Return device info (frontend stores token separately)
    return DeviceRegisterResponse(
        id=device.id,
        name=device.name,
        user_id=device.user_id,
    )


@router.get("/me", response_model=DeviceMeResponse)
def get_me(device: Device = Depends(get_device)):
    """Return the authenticated device's own record."""
    return DeviceMeResponse(
        id=device.id,
        name=device.name,
        user_id=device.user_id,
    )


@router.put("/me/poll_url", response_model=DeviceMeResponse)
def update_poll_url(
    body: DeviceUpdatePollUrl,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """Update the URL the backend should call for long-poll delivery."""
    device.poll_url = body.poll_url
    db.add(device)
    db.commit()
    db.refresh(device)
    return DeviceMeResponse(
        id=device.id,
        name=device.name,
        user_id=device.user_id,
    )
