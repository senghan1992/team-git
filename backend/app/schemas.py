"""Pydantic request/response schemas."""
from datetime import datetime
from pydantic import BaseModel, Field


# ── User / auth ───────────────────────────────────────────────────────────────


class UserPublic(BaseModel):
    """A user as the client is allowed to see them — never the password hash."""
    id: str
    username: str
    email: str
    name: str
    created_at: datetime


class RegisterRequest(BaseModel):
    name: str = Field(..., min_length=1, max_length=256)
    email: str = Field(..., min_length=3, max_length=256)
    username: str = Field(..., min_length=2, max_length=64)
    password: str = Field(..., min_length=8, max_length=256)


class LoginRequest(BaseModel):
    username: str = Field(..., min_length=1, max_length=256)
    password: str = Field(..., min_length=1, max_length=256)


class AuthResponse(BaseModel):
    """Returned by register and login: who you are + the token to send back."""
    user: UserPublic
    token: str


class GoogleUrlResponse(BaseModel):
    """The Google consent URL for one OAuth handshake."""
    url: str


class AuthConfigResponse(BaseModel):
    """로그인 화면이 어떤 방식들을 보여줄지 — 서버가 알려준다.

    `auth_mode` 는 운영자가 정한 값: `simple`(아이디+비밀번호만) 또는
    `google`(구글 버튼 추가). `google_enabled` 는 google 모드이면서 구글
    설정 3종(CLIENT_ID / CLIENT_SECRET / REDIRECT_URI)이 다 갖춰졌을 때만
    true — 앱은 이 값으로 "Google로 로그인" 버튼 표시 여부를 정한다.
    """
    auth_mode: str
    google_enabled: bool


class ProfileUpdateRequest(BaseModel):
    name: str | None = Field(default=None, min_length=1, max_length=256)
    email: str | None = Field(default=None, min_length=3, max_length=256)


class PasswordChangeRequest(BaseModel):
    current_password: str = Field(..., min_length=1, max_length=256)
    new_password: str = Field(..., min_length=8, max_length=256)


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
    """A push event from the desktop app's pre-push hook (device-authenticated)."""
    project_id: str
    event_kind: str = Field(..., pattern="^(main_push|branch_push|release)$")
    repo_name: str
    payload: str  # JSON string


class ServerHookRequest(BaseModel):
    """
    A push event from a git server-side hook (post-receive / webhook).

    앱을 설치하지 않은 팀원이 터미널·IDE 로 `git push` 해도 병합 관리자가
    알림을 받게 하기 위한 통로다. 기기 토큰 대신 서버 운영자가 정한 공유
    비밀키(`GC_HOOK_SECRET` 환경변수)로 인증한다.

    `payload` 는 앱 훅과 같은 JSON 문자열을 그대로 보낼 때 쓴다. 비어 있으면
    아래 필드들(author/message/sha/branch…)로 서버가 만든다 — 훅 스크립트가
    JSON 이스케이프를 직접 하지 않아도 되게 하기 위함이다.
    """
    project_id: str
    event_kind: str = Field(..., pattern="^(branch_push|main_push|release)$")
    repo_name: str
    payload: str = Field(default="")
    author: str | None = Field(default=None, max_length=256)
    author_email: str | None = Field(default=None, max_length=256)
    message: str | None = Field(default=None, max_length=2048)
    sha: str | None = Field(default=None, max_length=64)
    branch: str | None = Field(default=None, max_length=256)
    url: str | None = Field(default=None, max_length=512)
    version: str | None = Field(default=None, max_length=64)
    # 수신함에 보여 줄 이름 — 없으면 payload 의 author 를 쓴다.
    sender_name: str | None = Field(default=None, max_length=256)


class EventCreateResponse(BaseModel):
    id: str


class PollResponse(BaseModel):
    event: EventDetail | None = None


class AckRequest(BaseModel):
    event_id: str
