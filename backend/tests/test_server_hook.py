"""Tests for the git server-hook event channel (POST /events/server-hook).

앱을 설치하지 않은 팀원이 `git push` 해도 병합 관리자가 알림을 받는 통로다.
개별 기기 토큰 대신 서버 운영자의 공유 비밀키(GC_HOOK_SECRET)로 인증한다.
GitHub/GitLab 웹훅 페이로드도 이 엔드포인트가 직접 받는다.
"""
import hashlib
import hmac
import json
import os
import uuid

import pytest

from app.db import Session
from app.models import Device, EventDelivery, ProjectMember, PushEvent, User
from tests.conftest import make_device


def _secret() -> str:
    return "test-hook-secret-1234"


@pytest.fixture(autouse=True)
def hook_secret_env():
    os.environ["GC_HOOK_SECRET"] = _secret()
    yield
    os.environ.pop("GC_HOOK_SECRET", None)


def _make_project(client, token: str, join_tokens: list[str] | None = None) -> dict:
    resp = client.post(
        "/projects",
        json={"display_name": "팀 저장소"},
        headers={"Authorization": f"Bearer {token}"},
    )
    project = resp.json()
    for t in join_tokens or []:
        client.post(
            "/projects/join",
            json={"join_code": project["join_code"]},
            headers={"Authorization": f"Bearer {t}"},
        )
    return project


def _post_hook(
    client,
    project_id: str,
    *,
    secret: str | None = None,
    author_email: str | None = None,
    author: str = "김철수",
    kind: str = "branch_push",
    sha: str = "deadbeef",
    raw_payload: str | None = None,
) -> object:
    headers = {} if secret is None else {"X-Hook-Secret": secret}
    body = {
        "project_id": project_id,
        "event_kind": kind,
        "repo_name": "team-app",
        "author_email": author_email,
        "sender_name": author,
    }
    if raw_payload is not None:
        body["payload"] = raw_payload
    else:
        # 필드만 보내면 서버가 앱 훅과 같은 payload 를 만들어 준다
        body.update(
            {
                "author": author,
                "message": "feat: 로그인 추가",
                "sha": sha,
                "url": "git@server:team/team-app.git",
                "branch": "feature/login",
            }
        )
    return client.post(
        "/events/server-hook",
        json=body,
        headers=headers,
    )


def _add_member(db, project_id: str, device_id: str) -> None:
    db.add(ProjectMember(project_id=project_id, device_id=device_id, role="member"))
    db.commit()


def test_secret_missing_disables_endpoint(client):
    os.environ.pop("GC_HOOK_SECRET", None)  # 운영자가 키를 안 정한 상태
    resp = _post_hook(client, "any-project", secret="anything")
    assert resp.status_code == 404


def test_wrong_secret_rejected(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)
    resp = _post_hook(client, project["id"], secret="wrong-secret")
    assert resp.status_code == 403


def test_unknown_project_rejected(client):
    resp = _post_hook(client, uuid.uuid4().hex, secret=_secret())
    assert resp.status_code == 404


def test_hook_fans_out_to_all_members_when_author_is_pure_git_user(client):
    """앱에 없는 사람(순수 git 유저)이 push 하면 프로젝트 전원에게 배달한다."""
    db = Session()
    _, token_a = make_device(db, "device-a")
    _, token_b = make_device(db, "device-b")
    db.close()
    project = _make_project(client, token_a, join_tokens=[token_b])

    resp = _post_hook(client, project["id"], secret=_secret(), author_email=None)
    assert resp.status_code == 200
    event_id = resp.json()["id"]

    db = Session()
    deliveries = db.query(EventDelivery).filter(EventDelivery.event_id == event_id).all()
    db.close()
    assert len(deliveries) == 2  # device-a + device-b


def test_hook_excludes_author_by_device_name(client):
    """앱도 쓰는 팀원이 터미널로 push → 자기 기기에는 배달하지 않는다.

    기기 이름은 앱이 로그인한 사람의 표시 이름으로 등록하므로, payload 의
    author(= git config user.name)와 같으면 그 사람의 기기로 본다.
    """
    db = Session()
    _, token_a = make_device(db, "device-a")
    _, token_chulsoo = make_device(db, "김철수")  # 앱을 쓰는 팀원
    db.close()
    project = _make_project(client, token_a, join_tokens=[token_chulsoo])

    resp = _post_hook(
        client,
        project["id"],
        secret=_secret(),
        author_email=None,  # 서버 훅은 이메일을 못 알 수도 있다
        author="김철수",
    )
    assert resp.status_code == 200
    event_id = resp.json()["id"]

    db = Session()
    chulsoo_device = db.query(Device).filter(Device.name == "김철수").first()
    deliveries = db.query(EventDelivery).filter(EventDelivery.event_id == event_id).all()
    db.close()
    recipient_ids = [d.device_id for d in deliveries]
    assert chulsoo_device.id not in recipient_ids
    assert len(deliveries) == 1  # 관리자 기기(device-a)만


def test_hook_excludes_author_by_email_when_linked(client):
    """이메일→사용자→기기로 이어지는 경우 이메일로도 자기 기기를 제외한다."""
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a, join_tokens=[])

    # 이메일로 연결된 사용자+기기 (설계상 드문 케이스 — 미래 호환)
    db = Session()
    user = User(
        id=str(uuid.uuid4().hex),
        username="chulsoo",
        email="chulsoo@example.com",
        name="김철수",
        password_hash="x",
    )
    db.add(user)
    db.commit()
    linked = Device(
        id=str(uuid.uuid4().hex),
        user_id=user.id,
        name="김철수의 앱",
        token_hash="no-token",
    )
    db.add(linked)
    db.commit()
    linked_id = linked.id
    _add_member(db, project["id"], linked.id)
    db.close()

    resp = _post_hook(
        client,
        project["id"],
        secret=_secret(),
        author_email="chulsoo@example.com",
        author="김철수",
    )
    assert resp.status_code == 200
    event_id = resp.json()["id"]

    db = Session()
    deliveries = db.query(EventDelivery).filter(EventDelivery.event_id == event_id).all()
    db.close()
    recipient_ids = [d.device_id for d in deliveries]
    assert linked_id not in recipient_ids
    assert len(deliveries) == 1


def test_poll_response_shows_author_name_for_server_hook_events(client):
    """기기가 없는 이벤트는 payload 의 author 를 발신자 이름으로 보여 준다."""
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_hook(client, project["id"], secret=_secret())
    assert resp.status_code == 200
    event_id = resp.json()["id"]

    db = Session()
    from app.models import EventDelivery
    db.query(EventDelivery).filter(EventDelivery.event_id == event_id).first()
    db.close()

    # poll 로 받으면 발신자 이름이 payload author 로 채워져 있다
    db = Session()
    row = db.query(PushEvent).filter(PushEvent.id == event_id).first()
    from app.routes.events import _event_detail
    detail = _event_detail(db, row)
    db.close()
    assert detail.sender_device_name == "김철수"


def test_duplicate_sha_is_delivered_once(client):
    """앱 훅과 서버 훅이 같은 push 를 두 번 보내도 이벤트는 한 번만 생긴다."""
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    first = _post_hook(client, project["id"], secret=_secret(), sha="same-sha-123")
    second = _post_hook(client, project["id"], secret=_secret(), sha="same-sha-123")
    assert first.status_code == 200 and second.status_code == 200
    assert first.json()["id"] == second.json()["id"]

    db = Session()
    count = db.query(PushEvent).filter(PushEvent.project_id == project["id"]).count()
    db.close()
    assert count == 1

# ── GitHub 웹훅 ───────────────────────────────────────────────────────────

def _gh_payload(
    *,
    ref: str = "refs/heads/feature/login",
    after: str = "a" * 40,
    default_branch: str = "main",
    commit_author: str = "박영희",
    commit_email: str = "younghee@example.com",
) -> dict:
    return {
        "ref": ref,
        "before": "0" * 40,
        "after": after,
        "created": True,
        "deleted": False,
        "head_commit": {
            "id": after,
            "message": "feat: 알림 추가",
            "author": {"name": commit_author, "email": commit_email},
        },
        "repository": {
            "name": "team-app",
            "full_name": "org/team-app",
            "html_url": "https://github.com/org/team-app",
            "default_branch": default_branch,
        },
    }


def _post_github(
    client,
    project_id: str,
    payload: dict,
    *,
    secret: str | None = None,
    query: str = "",
    event: str = "push",
) -> object:
    body = json.dumps(payload).encode("utf-8")
    headers = {"X-GitHub-Event": event, "Content-Type": "application/json"}
    if secret is not None:
        sig = "sha256=" + hmac.new(secret.encode(), body, hashlib.sha256).hexdigest()
        headers["X-Hub-Signature-256"] = sig
    url = f"/events/server-hook?project_id={project_id}{query}"
    return client.post(url, content=body, headers=headers)


def _last_event(db) -> PushEvent:
    return db.query(PushEvent).order_by(PushEvent.created_at.desc()).first()


def test_github_webhook_branch_push_mapped(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_github(client, project["id"], _gh_payload(), secret=_secret())
    assert resp.status_code == 200

    db = Session()
    e = _last_event(db)
    db.close()
    assert e.event_kind == "branch_push"
    data = json.loads(e.payload)["data"]
    assert data["author"] == "박영희"
    assert data["message"] == "feat: 알림 추가"
    assert data["sha"] == "a" * 40
    assert data["branch"] == "feature/login"
    assert data["repo_name"] == "team-app"
    assert data["url"] == "https://github.com/org/team-app"


def test_github_webhook_default_branch_is_main_push(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_github(
        client,
        project["id"],
        _gh_payload(ref="refs/heads/main"),
        secret=_secret(),
    )
    assert resp.status_code == 200
    db = Session()
    assert _last_event(db).event_kind == "main_push"
    db.close()


def test_github_webhook_custom_merge_target_via_query(client):
    """기본 브랜치가 main 이어도 URL 에 merge_targets=develop 를 넣으면
    develop push 는 main_push(동기화 안내)로 분류된다."""
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_github(
        client,
        project["id"],
        _gh_payload(ref="refs/heads/develop"),
        secret=_secret(),
        query="&merge_targets=develop,release/1.0",
    )
    assert resp.status_code == 200
    db = Session()
    assert _last_event(db).event_kind == "main_push"
    db.close()

    # merge_targets 에 없으면 branch_push
    resp = _post_github(
        client,
        project["id"],
        _gh_payload(ref="refs/heads/feature/x"),
        secret=_secret(),
        query="&merge_targets=develop,release/1.0",
    )
    db = Session()
    assert _last_event(db).event_kind == "branch_push"
    db.close()


def test_github_webhook_tag_is_release(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_github(
        client,
        project["id"],
        _gh_payload(ref="refs/tags/v1.2.3", after="b" * 40),
        secret=_secret(),
    )
    assert resp.status_code == 200
    db = Session()
    e = _last_event(db)
    db.close()
    assert e.event_kind == "release"
    assert json.loads(e.payload)["data"]["version"] == "1.2.3"


def test_github_webhook_bad_signature_rejected(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_github(client, project["id"], _gh_payload(), secret="wrong-secret")
    assert resp.status_code == 403


def test_github_webhook_deletion_ignored(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    payload = _gh_payload()
    payload["deleted"] = True
    payload["after"] = "0" * 40
    resp = _post_github(client, project["id"], payload, secret=_secret())
    assert resp.status_code == 200
    assert resp.json()["id"] == ""
    db = Session()
    assert db.query(PushEvent).filter(PushEvent.project_id == project["id"]).count() == 0
    db.close()


def test_github_webhook_ping_answered_ok(client):
    resp = _post_github(client, "whatever", {}, event="ping")
    assert resp.status_code == 200


# ── GitLab 웹훅 ───────────────────────────────────────────────────────────

def _gl_payload(
    *,
    ref: str = "refs/heads/feature/login",
    after: str = "c" * 40,
    default_branch: str = "main",
) -> dict:
    return {
        "object_kind": "push",
        "before": "0" * 40,
        "after": after,
        "ref": ref,
        "checkout_sha": after,
        "user_name": "박영희",
        "user_email": "younghee@example.com",
        "project": {
            "name": "corelib",
            "default_branch": default_branch,
            "web_url": "http://mod.lge.com/hub/esdataplfm/part_de/corelib",
            "git_http_url": "http://mod.lge.com/hub/esdataplfm/part_de/corelib.git",
        },
        "repository": {"name": "corelib"},
        "commits": [
            {
                "id": after,
                "message": "fix: 빌드 오류 수정",
                "author": {"name": "박영희", "email": "younghee@example.com"},
            }
        ],
    }


def _post_gitlab(client, project_id: str, payload: dict, *, token: str | None = None) -> object:
    headers = {
        "X-Gitlab-Event": "Push Hook",
        "Content-Type": "application/json",
    }
    if token is not None:
        headers["X-Gitlab-Token"] = token
    return client.post(
        f"/events/server-hook?project_id={project_id}",
        json=payload,
        headers=headers,
    )


def test_gitlab_webhook_push_mapped(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_gitlab(client, project["id"], _gl_payload(), token=_secret())
    assert resp.status_code == 200

    db = Session()
    e = _last_event(db)
    db.close()
    assert e.event_kind == "branch_push"
    data = json.loads(e.payload)["data"]
    assert data["author"] == "박영희"
    assert data["message"] == "fix: 빌드 오류 수정"
    assert data["sha"] == "c" * 40
    assert data["branch"] == "feature/login"
    assert data["repo_name"] == "corelib"
    assert data["url"] == "http://mod.lge.com/hub/esdataplfm/part_de/corelib.git"


def test_gitlab_webhook_default_branch_is_main_push(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_gitlab(
        client,
        project["id"],
        _gl_payload(ref="refs/heads/main"),
        token=_secret(),
    )
    assert resp.status_code == 200
    db = Session()
    assert _last_event(db).event_kind == "main_push"
    db.close()


def test_gitlab_webhook_wrong_token_rejected(client):
    db = Session()
    _, token_a = make_device(db, "device-a")
    db.close()
    project = _make_project(client, token_a)

    resp = _post_gitlab(client, project["id"], _gl_payload(), token="wrong")
    assert resp.status_code == 403
