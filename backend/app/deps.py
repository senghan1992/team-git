"""FastAPI route dependencies."""
from datetime import datetime
from typing import Annotated

from fastapi import Depends, Header

from app.auth import hash_token, AuthError
from app.db import Session
from app.models import Device, User, UserSession


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


def bearer_token(authorization: str | None) -> str:
    """Pull the token out of an ``Authorization: Bearer <token>`` header."""
    if not authorization:
        raise AuthError("Missing Authorization header")
    parts = authorization.split(" ", 1)
    if len(parts) != 2 or parts[0].lower() != "bearer":
        raise AuthError("Invalid Authorization header format")
    return parts[1]


def get_user(
    authorization: Annotated[str | None, Header()] = None,
    db: Session = Depends(get_db),
) -> User:
    """
    Authenticate a *person* (not a device) from their login token.

    Device tokens and user tokens are separate: a device is one installation of
    the app, a user is the human. Login/profile endpoints need the human.
    """
    token_hash = hash_token(bearer_token(authorization))
    session = db.query(UserSession).filter(UserSession.token_hash == token_hash).first()
    if not session:
        raise AuthError("로그인이 필요합니다. 다시 로그인하세요.")
    user = db.query(User).filter(User.id == session.user_id).first()
    if not user:
        # 계정이 사라졌는데 토큰만 남은 경우 — 토큰도 정리한다.
        db.delete(session)
        db.commit()
        raise AuthError("계정을 찾을 수 없습니다. 다시 로그인하세요.")
    session.last_seen = datetime.utcnow()
    db.add(session)
    db.commit()
    return user
