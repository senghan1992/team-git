"""Pytest fixtures: app, client, and isolated database."""
import os
import tempfile
import pytest
from pathlib import Path

# Use a temp file so all connections share the same DB
_db_file = tempfile.mktemp(suffix=".db")
os.environ["GC_PEER_DB_URL"] = f"sqlite+pysqlite:///{_db_file}"

from fastapi.testclient import TestClient
from app.main import app
from app.db import engine, Base
from app.auth import generate_token, hash_token
from app.models import Device
import uuid


def make_device(db, name: str = "device") -> tuple[str, str]:
    """Create a Device in the given session. Returns (device_id, token)."""
    token = generate_token()
    device_id = str(uuid.uuid4().hex)
    user_id = str(uuid.uuid4().hex)
    db.add(Device(
        id=device_id,
        user_id=user_id,
        name=name,
        token_hash=hash_token(token),
    ))
    db.commit()
    return device_id, token


@pytest.fixture(scope="function")
def client():
    """Fresh schema for each test via create_all/drop_all."""
    Base.metadata.create_all(bind=engine)
    with TestClient(app) as c:
        yield c
    Base.metadata.drop_all(bind=engine)


@pytest.fixture
def db_session():
    """Provide a session for direct DB manipulation in tests."""
    from app.db import Session
    session = Session()
    yield session
    session.close()


@pytest.fixture
def auth_headers(client: TestClient) -> dict[str, str]:
    """Register a device, return Authorization header with its bearer token."""
    from app.db import Session
    token = generate_token()
    session = Session()
    try:
        device = Device(
            id=str(uuid.uuid4().hex),
            user_id=str(uuid.uuid4().hex),
            name="test-device",
            token_hash=hash_token(token),
        )
        session.add(device)
        session.commit()
    finally:
        session.close()
    return {"Authorization": f"Bearer {token}"}
