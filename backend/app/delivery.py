"""Long-poll delivery engine for peer push events."""
import asyncio
import httpx
from datetime import datetime

# In-memory map: device_id -> asyncio.Event (pure wakeup, no payload)
_waiters: dict[str, asyncio.Event] = {}
_lock = asyncio.Lock()

POLL_TIMEOUT = 25  # seconds


async def queue_event(event_id: str, project_id: str, sender_device_id: str) -> None:
    """
    Persist a delivery record for every project member except the sender.

    Opens its own database session so it is not tied to the request lifecycle.
    - Each subscriber (except the sender) gets an EventDelivery row with
      delivered_at=None (pending).
    - Subscribers with an active long-poll waiter are immediately pushed to;
      their row is updated to delivered_at=now on success.
    - Subscribers without an active waiter stay pending; they will be served on next poll.
    """
    from app.db import Session
    from app.models import ProjectMember, EventDelivery

    db = Session()
    try:
        members = (
            db.query(ProjectMember)
            .filter(
                ProjectMember.project_id == project_id,
                ProjectMember.device_id != sender_device_id,
            )
            .all()
        )

        for member in members:
            # Persist pending delivery record for every subscriber
            delivery = EventDelivery(
                event_id=event_id,
                device_id=member.device_id,
                delivered_at=None,
                acked_at=None,
            )
            db.add(delivery)

        db.commit()

        # Wake waiters and attempt immediate delivery
        for member in members:
            async with _lock:
                waiter = _waiters.get(member.device_id)

            if waiter is not None:
                waiter.set()
                await _deliver_with_retry(event_id, member.device_id, db)

    finally:
        db.close()


async def _deliver_with_retry(event_id: str, device_id: str, db) -> None:
    """
    Attempt HTTP delivery of event_id to device_id's poll_url.

    Loads the full PushEvent and sender Device, sends all fields so the
    sidecar can insert a useful team_events row.
    Updates EventDelivery.delivered_at on success.
    Retries up to 3x with exponential backoff (5 s / 15 s / 45 s).
    """
    from app.models import Device, EventDelivery, PushEvent

    device = db.query(Device).filter(Device.id == device_id).first()
    if not device or not device.poll_url:
        return

    event = db.query(PushEvent).filter(PushEvent.id == event_id).first()
    if not event:
        return

    sender = db.query(Device).filter(Device.id == event.sender_device_id).first()
    sender_name = sender.name if sender else "peer"

    poll_url = device.poll_url
    backoffs = [5, 15, 45]
    timeout = 0.2

    for attempt in range(4):
        try:
            async with httpx.AsyncClient() as client:
                resp = await client.post(
                    f"{poll_url}/events",
                    json={
                        "event_id": event_id,
                        "project_id": event.project_id,
                        "sender_device_name": sender_name,
                        "event_kind": event.event_kind,
                        "repo_name": event.repo_name,
                        "payload": event.payload,
                    },
                    timeout=timeout,
                )
                if resp.status_code == 200:
                    delivery = (
                        db.query(EventDelivery)
                        .filter(
                            EventDelivery.event_id == event_id,
                            EventDelivery.device_id == device_id,
                        )
                        .first()
                    )
                    if delivery and delivery.delivered_at is None:
                        delivery.delivered_at = datetime.utcnow()
                        db.add(delivery)
                        db.commit()
                    return
        except Exception:
            pass

        if attempt < 3:
            await asyncio.sleep(backoffs[attempt])
            timeout = min(timeout * 2, 5.0)

    # All retries failed -- delivery row stays pending; device will poll and get it


async def poll_event(device_id: str, timeout: int = POLL_TIMEOUT) -> str | None:
    """
    Pure wakeup -- returns as soon as a waiter event fires or timeout.

    The caller should then query the database for the next undelivered
    EventDelivery row for this device and fetch the full PushEvent.
    """
    event = asyncio.Event()

    async with _lock:
        existing = _waiters.get(device_id)
        if existing is not None:
            event = existing
        _waiters[device_id] = event

    try:
        await asyncio.wait_for(event.wait(), timeout=timeout)
        event.clear()
    except asyncio.TimeoutError:
        return None
    finally:
        async with _lock:
            _waiters.pop(device_id, None)

    return None
