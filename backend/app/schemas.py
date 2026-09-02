"""Pydantic request/response schemas."""
from datetime import datetime
from pydantic import BaseModel, Field


# ── Device ────────────────────────────────────────────────────────────────────


class DeviceRegisterRequest(BaseModel):
    name: str = Field(..., min_length=1, max_length=256)
    email: str | None = Field(default=None, max_length=256)


class DeviceRegisterResponse(BaseModel):
    id: str
    name: str
    user_id: str


class DeviceMeResponse(BaseModel):
    id: str
    name: str
    user_id: str


class DeviceUpdatePollUrl(BaseModel):
    poll_url: str | None


# ── Project ───────────────────────────────────────────────────────────────────


class ProjectCreateRequest(BaseModel):
    display_name: str = Field(..., min_length=1, max_length=256)


class ProjectCreateResponse(BaseModel):
    id: str
    display_name: str
    join_code: str
    role: str  # "owner"


class ProjectJoinRequest(BaseModel):
    join_code: str = Field(..., min_length=1)


class ProjectInfo(BaseModel):
    id: str
    display_name: str
    join_code: str
    role: str


class ProjectListResponse(BaseModel):
    projects: list[ProjectInfo]


# ── Member ─────────────────────────────────────────────────────────────────────


class MemberInfo(BaseModel):
    device_id: str | None = None
    email: str | None = None
    name: str | None = None
    role: str
    joined_at: datetime | None = None


class MemberListResponse(BaseModel):
    members: list[MemberInfo]


class MemberAddByEmailRequest(BaseModel):
    email: str = Field(..., min_length=1, max_length=256)
    name: str | None = None
    role: str = "member"


class MemberAddByEmailResponse(BaseModel):
    device_id: str | None = None
    email: str
    role: str
    pending: bool


# ── Events ────────────────────────────────────────────────────────────────────


class EventDetail(BaseModel):
    """Must appear before PollResponse since PollResponse.event references it."""
    id: str
    project_id: str
    sender_device_id: str
    sender_device_name: str | None = None
    event_kind: str
    repo_name: str
    payload: str
    created_at: datetime


class EventCreateRequest(BaseModel):
    project_id: str
    event_kind: str = Field(..., pattern="^(main_push|branch_push|release)$")
    repo_name: str
    payload: str  # JSON string


class EventCreateResponse(BaseModel):
    id: str


class PollResponse(BaseModel):
    event: EventDetail | None = None


class AckRequest(BaseModel):
    event_id: str
