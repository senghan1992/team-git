"""SQLAlchemy ORM models for the peer backend."""
from datetime import datetime
import uuid

from sqlalchemy import String, UniqueConstraint
from sqlalchemy.orm import Mapped, mapped_column

from app.db import Base


def _uuid() -> str:
    return uuid.uuid4().hex


class Device(Base):
    """Represents a registered desktop app instance."""
    __tablename__ = "devices"

    id: Mapped[str] = mapped_column(String(64), primary_key=True, default=_uuid)
    user_id: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    name: Mapped[str] = mapped_column(String(256), nullable=False)
    token_hash: Mapped[str] = mapped_column(String(64), nullable=False, unique=True)
    poll_url: Mapped[str | None] = mapped_column(String(512), nullable=True)
    last_seen: Mapped[datetime] = mapped_column(default=datetime.utcnow)
    created_at: Mapped[datetime] = mapped_column(default=datetime.utcnow)


class Project(Base):
    """A team project that groups repos and members."""
    __tablename__ = "projects"

    id: Mapped[str] = mapped_column(String(64), primary_key=True, default=_uuid)
    display_name: Mapped[str] = mapped_column(String(256), nullable=False)
    join_code: Mapped[str] = mapped_column(String(16), nullable=False, unique=True, index=True)
    created_by: Mapped[str] = mapped_column(String(64), nullable=False)
    created_at: Mapped[datetime] = mapped_column(default=datetime.utcnow)


class ProjectMember(Base):
    """Junction table linking devices to projects with a role."""
    __tablename__ = "project_members"

    project_id: Mapped[str] = mapped_column(String(64), primary_key=True)
    device_id: Mapped[str] = mapped_column(String(64), primary_key=True)
    role: Mapped[str] = mapped_column(String(32), nullable=False)  # owner | maintainer | member
    joined_at: Mapped[datetime] = mapped_column(default=datetime.utcnow)

    __table_args__ = (UniqueConstraint("project_id", "device_id"),)


class ProjectMemberEmail(Base):
    """Pending email invites for a project — auto-joined when the email holder registers."""
    __tablename__ = "project_member_emails"

    project_id: Mapped[str] = mapped_column(String(64), primary_key=True)
    email: Mapped[str] = mapped_column(String(256), primary_key=True)
    name: Mapped[str | None] = mapped_column(String(256), nullable=True)
    role: Mapped[str] = mapped_column(String(32), nullable=False, default="member")
    invited_at: Mapped[datetime] = mapped_column(default=datetime.utcnow)

    __table_args__ = (UniqueConstraint("project_id", "email"),)


class EventDelivery(Base):
    """
    Per-recipient delivery state for a PushEvent.

    One row per (event_id, device_id) pair.
    - None delivered_at: event is pending for this device (offline when event fired).
    - Set delivered_at: event was successfully pushed to the device's poll_url.
    - Set acked_at: device acknowledged receipt.
    """
    __tablename__ = "event_deliveries"

    event_id: Mapped[str] = mapped_column(String(64), primary_key=True)
    device_id: Mapped[str] = mapped_column(String(64), primary_key=True)
    delivered_at: Mapped[datetime | None] = mapped_column(nullable=True)
    acked_at: Mapped[datetime | None] = mapped_column(nullable=True)


class PushEvent(Base):
    """A push event fanned out to project subscribers."""
    __tablename__ = "push_events"

    id: Mapped[str] = mapped_column(String(64), primary_key=True, default=_uuid)
    project_id: Mapped[str] = mapped_column(String(64), nullable=False, index=True)
    sender_device_id: Mapped[str] = mapped_column(String(64), nullable=False)
    event_kind: Mapped[str] = mapped_column(String(32), nullable=False)  # main_push | branch_push | release
    repo_name: Mapped[str] = mapped_column(String(256), nullable=False)
    payload: Mapped[str] = mapped_column(String(8192), nullable=False)  # JSON-serialized
    created_at: Mapped[datetime] = mapped_column(default=datetime.utcnow)
