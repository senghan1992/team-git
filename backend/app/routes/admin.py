"""서버 운영자용 관리 API — 사용자·프로젝트의 동작성을 추적하고 제어한다.

권한은 users.is_admin 플래그 하나로 정해진다 (backend/adminctl.py 로 지정).
가입 순서나 프로젝트 소유와 무관하게, 서버를 띄운 사람이 직접 임명한다.
모든 엔드포인트는 로그인한 관리자 세션을 요구하며, 일반 사용자에게는 403.

추적(tracking): 사용자·기기·세션·프로젝트·이벤트의 현황과 최근 활동.
제어(control): 계정 정지/해제(세션 즉시 삭제), 강제 로그아웃, 프로젝트에서
멤버 제거, 프로젝트 삭제.
"""
import json
from datetime import datetime, timedelta
from typing import Annotated

from fastapi import APIRouter, Depends, Header, HTTPException, status
from pydantic import BaseModel

from app.auth import hash_token
from app.db import Session
from app.deps import get_db, get_user
from app.models import (
    Device,
    EventDelivery,
    Project,
    ProjectMember,
    ProjectMemberEmail,
    PushEvent,
    User,
    UserSession,
)

router = APIRouter()


# ── 가드 ────────────────────────────────────────────────────────────────────


def require_admin(
    authorization: Annotated[str | None, Header()] = None,
    db: Session = Depends(get_db),
) -> User:
    """로그인한 사용자 중 is_admin 플래그가 켜진 사람만 통과시킨다."""
    user = get_user(authorization=authorization, db=db)
    if not user.is_admin:
        raise HTTPException(
            status.HTTP_403_FORBIDDEN,
            "관리자 계정이 아닙니다. 서버 운영자에게 문의하세요.",
        )
    return user


# ── 조각들 ──────────────────────────────────────────────────────────────────


def _iso(dt: datetime | None) -> str | None:
    return dt.isoformat() if dt else None


def _user_last_seen(db: Session, user_id: str) -> datetime | None:
    """세션과 기기의 last_seen 중 가장 최근 것 — '마지막으로 접속한 시각'."""
    stamps: list[datetime] = []
    for row in db.query(UserSession).filter(UserSession.user_id == user_id):
        if row.last_seen:
            stamps.append(row.last_seen)
    for row in db.query(Device).filter(Device.user_id == user_id):
        if row.last_seen:
            stamps.append(row.last_seen)
    return max(stamps) if stamps else None


def _user_summary(db: Session, u: User) -> dict:
    sessions = db.query(UserSession).filter(UserSession.user_id == u.id).count()
    devices = db.query(Device).filter(Device.user_id == u.id).all()
    device_ids = [d.id for d in devices]
    project_count = (
        db.query(ProjectMember.project_id)
        .filter(ProjectMember.device_id.in_(device_ids))
        .distinct()
        .count()
        if device_ids
        else 0
    )
    return {
        "id": u.id,
        "username": u.username,
        "email": u.email,
        "name": u.name,
        "is_admin": bool(u.is_admin),
        "disabled": bool(u.disabled),
        "created_at": _iso(u.created_at),
        "last_login_at": _iso(u.last_login_at),
        "last_seen": _iso(_user_last_seen(db, u.id)),
        "sessions": sessions,
        "devices": len(devices),
        "projects": project_count,
    }


def _project_summary(db: Session, p: Project) -> dict:
    # LEFT JOIN — 시드 데이터 등에서 기기의 user_id 가 users 테이블에 없는
    # 고아 기기가 있어도 멤버 목록이 통째로 사라지지 않게 한다.
    members = (
        db.query(ProjectMember, Device, User)
        .join(Device, ProjectMember.device_id == Device.id)
        .join(User, Device.user_id == User.id, isouter=True)
        .filter(ProjectMember.project_id == p.id)
        .all()
    )
    events = db.query(PushEvent).filter(PushEvent.project_id == p.id)
    total = events.count()
    day_ago = datetime.utcnow() - timedelta(hours=24)
    recent = events.filter(PushEvent.created_at >= day_ago).count()
    last = events.order_by(PushEvent.created_at.desc()).first()
    return {
        "id": p.id,
        "display_name": p.display_name,
        "created_at": _iso(p.created_at),
        "members": [
            {
                "device_id": m.Device.id,
                "device_name": m.Device.name,
                "role": m.ProjectMember.role,
                "user_name": m.User.name if m.User else None,
                "email": m.User.email if m.User else None,
                "last_seen": _iso(m.Device.last_seen),
            }
            for m in members
        ],
        "member_count": len(members),
        "events_total": total,
        "events_24h": recent,
        "last_event_at": _iso(last.created_at) if last else None,
    }


# ── 조회 (tracking) ─────────────────────────────────────────────────────────


@router.get("/overview")
def overview(
    admin: User = Depends(require_admin),
    db: Session = Depends(get_db),
):
    """관리 화면 첫 줄 — 서버 전체의 규모와 최근 활동량."""
    day_ago = datetime.utcnow() - timedelta(hours=24)
    week_ago = datetime.utcnow() - timedelta(days=7)
    events = db.query(PushEvent)
    last = events.order_by(PushEvent.created_at.desc()).first()
    return {
        "users": db.query(User).count(),
        "disabled_users": db.query(User).filter(User.disabled.is_(True)).count(),
        "sessions": db.query(UserSession).count(),
        "devices": db.query(Device).count(),
        "projects": db.query(Project).count(),
        "events_24h": events.filter(PushEvent.created_at >= day_ago).count(),
        "events_7d": events.filter(PushEvent.created_at >= week_ago).count(),
        "last_event_at": _iso(last.created_at) if last else None,
        "generated_at": _iso(datetime.utcnow()),
    }


@router.get("/users")
def list_users(
    admin: User = Depends(require_admin),
    db: Session = Depends(get_db),
):
    """사용자 목록 — 활동 흔적(세션·기기·프로젝트·마지막 접속)과 함께."""
    users = db.query(User).order_by(User.created_at.asc()).all()
    return [_user_summary(db, u) for u in users]


@router.get("/projects")
def list_projects(
    admin: User = Depends(require_admin),
    db: Session = Depends(get_db),
):
    """프로젝트 목록 — 멤버(기기·사람)와 이벤트 활동량을 함께."""
    projects = db.query(Project).order_by(Project.created_at.asc()).all()
    return [_project_summary(db, p) for p in projects]


@router.get("/events")
def list_events(
    limit: int = 60,
    admin: User = Depends(require_admin),
    db: Session = Depends(get_db),
):
    """최근 이벤트 피드 — 누가 어느 저장소에 무엇을 했는지 시간순으로."""
    limit = max(1, min(limit, 200))
    rows = (
        db.query(PushEvent, Device, User)
        .join(Device, PushEvent.sender_device_id == Device.id, isouter=True)
        .join(User, Device.user_id == User.id, isouter=True)
        .order_by(PushEvent.created_at.desc())
        .limit(limit)
        .all()
    )
    out = []
    for ev, device, user in rows:
        message = ""
        author = ""
        try:
            data = (json.loads(ev.payload) or {}).get("data", {})
            message = str(data.get("message") or "")
            author = str(data.get("author") or "")
        except Exception:
            pass
        out.append(
            {
                "id": ev.id,
                "kind": ev.event_kind,
                "repo_name": ev.repo_name,
                "created_at": _iso(ev.created_at),
                "author": author,
                "message": message,
                "sender_device": device.name if device else "(삭제된 기기)",
                "sender_user": user.name if user else None,
            }
        )
    return out


# ── 제어 (control) ──────────────────────────────────────────────────────────


class UserStatusBody(BaseModel):
    disabled: bool


@router.post("/users/{user_id}/status")
def set_user_status(
    user_id: str,
    body: UserStatusBody,
    admin: User = Depends(require_admin),
    db: Session = Depends(get_db),
):
    """계정 정지/해제. 정지하면 세션을 즉시 모두 지워 로그아웃시킨다.

    자기 자신은 정지할 수 없다 — 유일한 관리자가 스스로 문을 잠그면
    되돌릴 방법이 CLI 하나뿐이 된다.
    """
    user = db.query(User).filter(User.id == user_id).first()
    if not user:
        raise HTTPException(status.HTTP_404_NOT_FOUND, "사용자를 찾을 수 없습니다.")
    if user.id == admin.id:
        raise HTTPException(status.HTTP_400_BAD_REQUEST, "자기 자신의 계정은 정지할 수 없습니다.")
    user.disabled = body.disabled
    revoked = 0
    if body.disabled:
        for s in db.query(UserSession).filter(UserSession.user_id == user_id):
            db.delete(s)
            revoked += 1
    db.add(user)
    db.commit()
    return {"ok": True, "disabled": bool(user.disabled), "sessions_revoked": revoked}


@router.post("/users/{user_id}/logout")
def force_logout(
    user_id: str,
    admin: User = Depends(require_admin),
    db: Session = Depends(get_db),
):
    """강제 로그아웃 — 그 사람의 모든 세션 토큰을 지운다 (기기는 남는다)."""
    user = db.query(User).filter(User.id == user_id).first()
    if not user:
        raise HTTPException(status.HTTP_404_NOT_FOUND, "사용자를 찾을 수 없습니다.")
    revoked = 0
    for s in db.query(UserSession).filter(UserSession.user_id == user_id):
        db.delete(s)
        revoked += 1
    db.commit()
    return {"ok": True, "sessions_revoked": revoked}


@router.delete("/projects/{project_id}/members/{device_id}")
def remove_member(
    project_id: str,
    device_id: str,
    admin: User = Depends(require_admin),
    db: Session = Depends(get_db),
):
    """프로젝트에서 기기(사람)를 제거한다 — 더 이상 이벤트를 받지 못한다."""
    row = (
        db.query(ProjectMember)
        .filter(ProjectMember.project_id == project_id, ProjectMember.device_id == device_id)
        .first()
    )
    if not row:
        raise HTTPException(status.HTTP_404_NOT_FOUND, "멤버를 찾을 수 없습니다.")
    if row.role == "owner":
        raise HTTPException(status.HTTP_400_BAD_REQUEST, "프로젝트 소유자는 제거할 수 없습니다.")
    # 이 기기가 아직 받지 못한 이 프로젝트 이벤트의 배달 예정분도 치운다.
    event_ids = [
        e.id
        for e in db.query(PushEvent.id).filter(PushEvent.project_id == project_id)
    ]
    if event_ids:
        db.query(EventDelivery).filter(
            EventDelivery.device_id == device_id,
            EventDelivery.event_id.in_(event_ids),
        ).delete(synchronize_session=False)
    db.delete(row)
    db.commit()
    return {"ok": True}


@router.delete("/projects/{project_id}")
def delete_project(
    project_id: str,
    admin: User = Depends(require_admin),
    db: Session = Depends(get_db),
):
    """프로젝트 삭제 — 멤버·초대·이벤트·배달 기록을 함께 치운다.

    저장소의 .gpconfig 나 git ref 는 건드리지 않는다(서버의 배달 조직만
    지운다). 되돌릴 수 없으므로 클라이언트에서 확인을 받는다.
    """
    project = db.query(Project).filter(Project.id == project_id).first()
    if not project:
        raise HTTPException(status.HTTP_404_NOT_FOUND, "프로젝트를 찾을 수 없습니다.")
    event_ids = [e.id for e in db.query(PushEvent.id).filter(PushEvent.project_id == project_id)]
    if event_ids:
        db.query(EventDelivery).filter(EventDelivery.event_id.in_(event_ids)).delete(
            synchronize_session=False
        )
    db.query(PushEvent).filter(PushEvent.project_id == project_id).delete(
        synchronize_session=False
    )
    db.query(ProjectMember).filter(ProjectMember.project_id == project_id).delete(
        synchronize_session=False
    )
    db.query(ProjectMemberEmail).filter(ProjectMemberEmail.project_id == project_id).delete(
        synchronize_session=False
    )
    db.delete(project)
    db.commit()
    return {"ok": True}
