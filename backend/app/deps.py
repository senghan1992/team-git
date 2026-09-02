"""FastAPI route dependencies."""
from datetime import datetime
from typing import Annotated

from fastapi import Depends, Header

from app.auth import hash_token, AuthError
from app.db import Session
from app.models import Device


def get_db():
    """Database session dependency."""
    db = Session()
    try:
        yield db
    finally:
        db.close()


def get_device(
    authorization: Annotated[str | None, Header()] = None,
    db: Session = Depends(get_db),
) -> Device:
    """
    Authenticate a request using the Bearer token.

    Extracts the token from ``Authorization: Bearer <token>``, looks up the
    device by token hash, and returns the Device ORM object.
    Raises 401 if the header is missing, malformed, or the token is unknown.
    """
    if not authorization:
        raise AuthError("Missing Authorization header")

    parts = authorization.split(" ", 1)
    if len(parts) != 2 or parts[0].lower() != "bearer":
        raise AuthError("Invalid Authorization header format")

    token = parts[1]
    token_hash = hash_token(token)

    device = db.query(Device).filter(Device.token_hash == token_hash).first()
    if not device:
        raise AuthError("Unknown device token")

    # Update last_seen
    device.last_seen = datetime.utcnow()
    db.add(device)
    db.commit()

    return device
