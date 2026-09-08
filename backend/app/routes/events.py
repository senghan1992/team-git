"""Event creation, polling, and acknowledgement endpoints."""
import asyncio
import hashlib
import hmac
import json
import os
import secrets
from datetime import datetime, timedelta
from typing import Annotated

from fastapi import APIRouter, Depends, Header, HTTPException, Request
from sqlalchemy import func
from sqlalchemy.orm import Session

from app.delivery import queue_event, poll_event
from app.deps import get_db, get_device
from app.models import Device, EventDelivery, Project, ProjectMember, PushEvent, User
from app.schemas import (
    EventCreateRequest,
    EventCreateResponse,
    EventDetail,
    PollResponse,
    ServerHookRequest,
)

router = APIRouter()

# Long-poll wait timeout in seconds
POLL_TIMEOUT = 25


def _event_detail(db: Session, row: PushEvent) -> EventDetail:
    """Poll responses must carry the sender's *name* like the push path does —
    clients were showing the raw hex device id as the sender otherwise.

    서버 훅으로 들어온 이벤트(기기 없음)는 payload 의 author 를 이름으로
    쓴다 — 안 그러면 수신함 발신자가 빈 칸/기기 id 로 보인다.
    """
    sender = None
    if row.sender_device_id and row.sender_device_id != "-":
        sender = db.query(Device).filter(Device.id == row.sender_device_id).first()
    name = sender.name if sender else None
    if not name:
        try:
            name = json.loads(row.payload).get("data", {}).get("author") or None
        except Exception:
            name = None
    return EventDetail(
        id=row.id,
        project_id=row.project_id,
        sender_device_id=row.sender_device_id,
        sender_device_name=name,
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
    db.flush()  # event.id 확보

    # 배달 레코드는 이벤트와 **같은 트랜잭션**에서 만든다 — 예전에는
    # create_task(queue_event)가 레코드를 커밋하기 전에 서버가 죽으면
    # PushEvent만 남고 배달 0건이라, 그 push 알림은 아무에게도 가지 않았다.
    members = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == body.project_id,
            ProjectMember.device_id != device.id,
        )
        .all()
    )
    for member in members:
        db.add(
            EventDelivery(
                event_id=event.id,
                device_id=member.device_id,
                delivered_at=None,
                acked_at=None,
            )
        )
    db.commit()
    db.refresh(event)

    # 웨이터 깨우기 + 즉시 푸시만 비동기로 (레코드는 이미 커밋됨).
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


def _github_signature_ok(raw: bytes, secret: str, header: str | None) -> bool:
    """GitHub 웹훅 서명(X-Hub-Signature-256: sha256=…) 검증 — HMAC-SHA256."""
    if not header or not header.startswith("sha256="):
        return False
    expected = hmac.new(secret.encode("utf-8"), raw, hashlib.sha256).hexdigest()
    return secrets.compare_digest(expected, header[len("sha256=") :].strip())


def _webhook_to_request(
    data: dict,
    *,
    source: str,  # "github" | "gitlab"
    project_id: str,
    merge_targets: list[str],
) -> "ServerHookRequest | None":
    """GitHub/GitLab 웹훅 JSON 을 내부 요청 형식(ServerHookRequest)으로 바꾼다.

    None → 알릴 필요가 없는 이벤트 (브랜치/태그 삭제 push, 이상한 ref 등).

    병합 대상 판정: URL 쿼리 `merge_targets`(쉼표 목록)가 있으면 그것을
    우선하고, 없으면 저장소 기본 브랜치(default branch)와 비교한다.
    저장소 안 .gpconfig 는 서버가 읽을 수 없으므로, 기본 브랜치와 다른
    병합 대상(예: develop)이 있으면 웹훅 URL 에 merge_targets 를 넣는다.
    """
    ref = data.get("ref") or ""
    after = str(data.get("after") or "")
    if after and set(after) == {"0"}:
        return None  # ref 삭제 push

    if source == "github":
        repo = data.get("repository") or {}
        commit = data.get("head_commit") or {}
        author = commit.get("author") or {}
        repo_name = repo.get("name") or ""
        default_branch = repo.get("default_branch") or ""
        url = repo.get("html_url") or ""
        message = commit.get("message") or ""
    else:  # gitlab
        project = data.get("project") or {}
        commits = data.get("commits") or []
        last = commits[-1] if commits else {}
        author = last.get("author") or {}
        repo_name = project.get("name") or ""
        default_branch = project.get("default_branch") or ""
        url = (
            project.get("git_http_url")
            or (data.get("repository") or {}).get("git_http_url")
            or project.get("web_url")
            or ""
        )
        message = last.get("message") or ""

    if ref.startswith("refs/tags/"):
        tag = ref[len("refs/tags/") :]
        version = tag[1:] if tag.startswith("v") else tag
        return ServerHookRequest(
            project_id=project_id,
            event_kind="release",
            repo_name=repo_name,
            author=author.get("name"),
            author_email=author.get("email"),
            version=version,
            url=url,
        )

    if not ref.startswith("refs/heads/"):
        return None  # refs/notes 등 알릴 필요 없는 ref

    branch = ref[len("refs/heads/") :]
    if branch in merge_targets or (not merge_targets and branch == default_branch):
        kind = "main_push"
    else:
        kind = "branch_push"
    return ServerHookRequest(
        project_id=project_id,
        event_kind=kind,
        repo_name=repo_name,
        author=author.get("name"),
        author_email=author.get("email"),
        message=message,
        sha=after,
        branch=branch,
        url=url,
    )


@router.post("/server-hook", response_model=EventCreateResponse)
async def create_event_from_server_hook(
    request: Request,
    x_hook_secret: Annotated[str | None, Header()] = None,
    db: Session = Depends(get_db),
):
    """
    앱 미설치 팀원의 push 를 알려주는 공용 진입점. 통로가 셋이다:

    ① X-Hook-Secret 헤더 + JSON/form — 개발자 테스트와 bash post-receive 훅
       (scripts/push-alert/). form 은 JSON 이스케이프 없이 필드만 보낸다.
    ② GitHub 웹훅 — X-GitHub-Event: push, X-Hub-Signature-256(HMAC) 검증.
       웹훅 URL 에 ?project_id=…&merge_targets=… 을 붙인다.
    ③ GitLab 웹훅 — X-Gitlab-Event: Push Hook, X-Gitlab-Token(평문) 검증.
       URL 은 ②와 같다.

    검증은 공통으로 서버 운영자가 정한 GC_HOOK_SECRET 환경변수를 쓴다
    (GitHub 는 웹훅 Secret 칸, GitLab 은 Secret token 칸에 같은 값을 넣는다).
    배달 대상은 일반 이벤트와 같다: 프로젝트 전원(앱을 켠 기기)의 폴링으로
    도착하고, 역할 라우팅(병합 관리자인지 등)은 각자의 앱이 한다.

    같은 push 가 앱 훅과 웹훅 둘 다로 들어올 수 있으므로(머지 관리자가 앱을
    쓰는 경우) 같은 sha 의 이벤트가 이미 있으면 중복 배달하지 않는다.
    """
    secret = os.environ.get("GC_HOOK_SECRET", "").strip()
    if not secret:
        raise HTTPException(
            status_code=404,
            detail="server hook disabled — GC_HOOK_SECRET 환경변수를 설정하세요",
        )

    headers = request.headers
    event = (headers.get("x-github-event") or "").lower()
    if event == "ping":
        # GitHub 웹훅 등록 화면의 "테스트" 버튼(ping) — 성공으로 답만 한다
        return EventCreateResponse(id="")

    raw = await request.body()
    channel = "github" if event == "push" else ""
    if not channel and (headers.get("x-gitlab-event") or "").lower() == "push hook":
        channel = "gitlab"

    if channel == "github":
        if not _github_signature_ok(raw, secret, headers.get("x-hub-signature-256")):
            raise HTTPException(status_code=403, detail="invalid hook secret")
        try:
            data = json.loads(raw.decode("utf-8", errors="replace"))
        except Exception as exc:
            raise HTTPException(status_code=422, detail="invalid body") from exc
        body = _webhook_to_request(
            data,
            source="github",
            project_id=request.query_params.get("project_id") or "",
            merge_targets=[
                t.strip()
                for t in (request.query_params.get("merge_targets") or "").split(",")
                if t.strip()
            ],
        )
    elif channel == "gitlab":
        if not secrets.compare_digest(headers.get("x-gitlab-token") or "", secret):
            raise HTTPException(status_code=403, detail="invalid hook secret")
        try:
            data = json.loads(raw.decode("utf-8", errors="replace"))
        except Exception as exc:
            raise HTTPException(status_code=422, detail="invalid body") from exc
        body = _webhook_to_request(
            data,
            source="gitlab",
            project_id=request.query_params.get("project_id") or "",
            merge_targets=[
                t.strip()
                for t in (request.query_params.get("merge_targets") or "").split(",")
                if t.strip()
            ],
        )
    else:
        # ① 기존 통로 — X-Hook-Secret (bash post-receive 훅 / 개발자 테스트)
        if not x_hook_secret or not secrets.compare_digest(x_hook_secret, secret):
            raise HTTPException(status_code=403, detail="invalid hook secret")
        try:
            text = raw.decode("utf-8", errors="replace")
            if "application/json" in (headers.get("content-type") or ""):
                body = ServerHookRequest.model_validate(json.loads(text))
            else:
                # form-urlencoded — python-multipart 의존성 없이 직접 파싱한다
                from urllib.parse import parse_qs

                pairs = parse_qs(text, keep_blank_values=True)
                body = ServerHookRequest.model_validate(
                    {k: v[0] for k, v in pairs.items()}
                )
        except Exception as exc:
            raise HTTPException(status_code=422, detail="invalid body") from exc

    if body is None:
        # 알릴 필요 없는 ref (삭제 push 등) — 조용히 성공으로 답한다
        return EventCreateResponse(id="")

    project = db.query(Project).filter(Project.id == body.project_id).first()
    if not project:
        raise HTTPException(status_code=404, detail="project not found")

    # ── payload 구성 ──────────────────────────────────────────────────────
    # 훅 스크립트가 JSON 이스케이프를 직접 하지 않아도 되게, 필드만 받아
    # 서버가 앱 훅과 같은 형식의 payload 를 만든다 (빈 문자열은 그대로 둔다 —
    # 앱 훅도 누락 필드를 ""로 채운다).
    payload = body.payload
    if not payload:
        if body.event_kind == "release":
            data = {
                "author": body.author or "",
                "repo_name": body.repo_name,
                "url": body.url or "",
                "version": body.version or "",
            }
        else:
            data = {
                "author": body.author or "",
                "message": body.message or "",
                "sha": body.sha or "",
                "repo_name": body.repo_name,
                "url": body.url or "",
                "branch": body.branch or "",
            }
        payload = json.dumps({"kind": body.event_kind, "data": data}, ensure_ascii=False)

    # ── 중복 방지 ─────────────────────────────────────────────────────────
    # 앱 훅도 보내고 서버 훅도 보내는 환경에서 같은 push 가 두 번 배달되면
    # 수신함에 같은 카드가 두 장 쌓인다. payload 의 sha 로 최근 3분 안의
    # 동일 이벤트를 찾아 이미 있으면 그 id 를 돌려준다.
    try:
        sha = json.loads(payload).get("data", {}).get("sha") or None
    except Exception:
        sha = None
    if sha:
        # 최근(3분) 같은 프로젝트·종류의 이벤트를 읽어 sha 를 직접 비교한다 —
        # payload 는 앱(compact)과 훅(python)이 직렬화 형식이 달라 LIKE 는 못 쓴다.
        recent = (
            db.query(PushEvent)
            .filter(
                PushEvent.project_id == body.project_id,
                PushEvent.event_kind == body.event_kind,
                PushEvent.created_at >= datetime.utcnow() - timedelta(minutes=3),
            )
            .all()
        )
        for ev in recent:
            try:
                if json.loads(ev.payload).get("data", {}).get("sha") == sha:
                    return EventCreateResponse(id=ev.id)
            except Exception:
                continue

    # ── 발신 기기 추정 (자기 알림 제외) ────────────────────────────────────
    # push 한 사람이 앱도 쓰는 팀원이면 그 사람의 기기에는 배달하지 않는다
    # (앱 훅 경로와 같은 자가 수신 방지). 두 단계로 찾는다:
    #   ① 이메일→사용자→기기  (계정과 기기가 연결된 경우)
    #   ② 기기 이름(payload 의 author) 근사 매칭 — 앱은 등록할 때 로그인한
    #      사람의 표시 이름을 기기 이름으로 쓴다.
    # 둘 다 못 찾으면(순수 git 유저) "-" 로 두고 전원에게 배달한다.
    sender_device_id = "-"
    author_name = None
    try:
        author_name = json.loads(payload).get("data", {}).get("author") or None
    except Exception:
        author_name = None

    if body.author_email:
        user = (
            db.query(User)
            .filter(User.email == body.author_email.strip().lower())
            .first()
        )
        if user:
            sender = (
                db.query(Device)
                .join(ProjectMember, ProjectMember.device_id == Device.id)
                .filter(
                    Device.user_id == user.id,
                    ProjectMember.project_id == body.project_id,
                )
                .first()
            )
            if sender:
                sender_device_id = sender.id

    if sender_device_id == "-" and author_name:
        sender = (
            db.query(Device)
            .join(ProjectMember, ProjectMember.device_id == Device.id)
            .filter(
                func.lower(Device.name) == author_name.strip().lower(),
                ProjectMember.project_id == body.project_id,
            )
            .first()
        )
        if sender:
            sender_device_id = sender.id

    # 이벤트 + 배달 레코드를 **같은 트랜잭션**에서 만든다 — 일반 create_event
    # 와 같은 이유로, 서버가 죽어도 배달 0건인 이벤트가 남지 않게 한다.
    event = PushEvent(
        project_id=body.project_id,
        sender_device_id=sender_device_id,
        event_kind=body.event_kind,
        repo_name=body.repo_name,
        payload=payload,
    )
    db.add(event)
    db.flush()

    members = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == body.project_id,
            ProjectMember.device_id != sender_device_id,
        )
        .all()
    )
    for member in members:
        db.add(
            EventDelivery(
                event_id=event.id,
                device_id=member.device_id,
                delivered_at=None,
                acked_at=None,
            )
        )
    db.commit()
    db.refresh(event)

    # 웨이터 깨우기 + 즉시 푸시만 비동기로 (레코드는 이미 커밋됨).
    asyncio.create_task(queue_event(event.id, body.project_id, sender_device_id))

    return EventCreateResponse(id=event.id)


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
