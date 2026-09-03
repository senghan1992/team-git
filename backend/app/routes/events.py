"""Event creation, polling, and acknowledgement endpoints."""
import asyncio
from datetime import datetime
from fastapi import APIRouter, Depends, HTTPException
from sqlalchemy.orm import Session

from app.delivery import queue_event, poll_event
from app.deps import get_db, get_device
from app.models import Device, Project, ProjectMember, PushEvent
from app.schemas import (
    EventCreateRequest,
    EventCreateResponse,
    EventDetail,
    PollResponse,
)

router = APIRouter()

# Long-poll wait timeout in seconds
POLL_TIMEOUT = 25


def _event_detail(db: Session, row: PushEvent) -> EventDetail:
    """Poll responses must carry the sender's *name* like the push path does —
    clients were showing the raw hex device id as the sender otherwise."""
    sender = db.query(Device).filter(Device.id == row.sender_device_id).first()
    return EventDetail(
        id=row.id,
        project_id=row.project_id,
        sender_device_id=row.sender_device_id,
        sender_device_name=sender.name if sender else None,
        event_kind=row.event_kind,
        repo_name=row.repo_name,
        payload=row.payload,
        created_at=row.created_at,
    )


@router.post("", response_model=EventCreateResponse)
async def create_event(
    body: EventCreateRequest,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """
    Create and fan out a push event to all project subscribers.

    The event is persisted, then delivered to each subscriber's active
    long-poll waiter (if any). Devices without an active waiter will
    pick up the event on their next poll.
    """
    # Verify sender is a member of the project
    membership = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == body.project_id,
            ProjectMember.device_id == device.id,
        )
        .first()
    )
    if not membership:
        raise HTTPException(status_code=403, detail="Not a member of this project")

    # Persist the event
    event = PushEvent(
        project_id=body.project_id,
        sender_device_id=device.id,
        event_kind=body.event_kind,
        repo_name=body.repo_name,
        payload=body.payload,
    )
    db.add(event)
    db.commit()
    db.refresh(event)

    # Fan out to all subscribers asynchronously
    asyncio.create_task(queue_event(event.id, body.project_id, device.id))

    return EventCreateResponse(id=event.id)


@router.post("/poll")
async def poll_events(
    wait: int = POLL_TIMEOUT,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """
    Long-poll for the next pending event for this device.

    Returns the next undelivered EventDelivery row for this device.
    Waits up to ``wait`` seconds if none are immediately available.
    """
    from app.models import EventDelivery

    # Fast path: return immediately if a pending delivery record already exists
    delivery = (
        db.query(EventDelivery)
        .filter(
            EventDelivery.device_id == device.id,
            EventDelivery.delivered_at.is_(None),
        )
        .join(PushEvent, EventDelivery.event_id == PushEvent.id)
        .order_by(PushEvent.created_at)
        .first()
    )
    if delivery:
        row = db.query(PushEvent).filter(PushEvent.id == delivery.event_id).first()
        if row:
            delivery.delivered_at = datetime.utcnow()
            db.add(delivery)
            db.commit()
            return PollResponse(event=_event_detail(db, row))

    # Slow path: wait for a new event to be queued
    await poll_event(device.id, wait)

    # Re-query after wakeup -- expire the session to see rows committed by queue_event's session
    db.expire_all()
    delivery = (
        db.query(EventDelivery)
        .filter(
            EventDelivery.device_id == device.id,
            EventDelivery.delivered_at.is_(None),
        )
        .join(PushEvent, EventDelivery.event_id == PushEvent.id)
        .order_by(PushEvent.created_at)
        .first()
    )
    if not delivery:
        return PollResponse(event=None)

    row = db.query(PushEvent).filter(PushEvent.id == delivery.event_id).first()
    if not row:
        return PollResponse(event=None)

    delivery.delivered_at = datetime.utcnow()
    db.add(delivery)
    db.commit()

    return PollResponse(event=_event_detail(db, row))


@router.post("/{event_id}/ack")
def ack_event(
    event_id: str,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """Acknowledge receipt of an event (marks first-ack timestamp on the delivery row)."""
    from app.models import EventDelivery

    # Verify the event exists
    event = db.query(PushEvent).filter(PushEvent.id == event_id).first()
    if not event:
        raise HTTPException(status_code=404, detail="Event not found")

    # Verify device is a member of the project
    membership = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == event.project_id,
            ProjectMember.device_id == device.id,
        )
        .first()
    )
    if not membership:
        raise HTTPException(status_code=403, detail="Not a member of this project")

    # Write ack on the delivery row
    delivery = (
        db.query(EventDelivery)
        .filter(
            EventDelivery.event_id == event_id,
            EventDelivery.device_id == device.id,
        )
        .first()
    )
    if delivery and delivery.acked_at is None:
        delivery.acked_at = datetime.utcnow()
        db.add(delivery)
        db.commit()

    return {"ok": True}
