"""관리자 API 테스트 — 권한 가드와 제어 동작(정지·로그아웃·멤버 제거·삭제)."""
import uuid

from app.db import Session
from app.models import User
from app.auth import hash_password

from .conftest import make_device


def _register(client, username: str) -> dict:
    r = client.post(
        "/auth/register",
        json={
            "username": username,
            "email": f"{username}@example.com",
            "name": username,
            "password": "pw-123456",
        },
    )
    assert r.status_code == 201, r.text
    return r.json()


def _make_admin(db: Session, user_id: str) -> None:
    """관리자 임명 — 운영자가 adminctl.py 로 하는 일을 테스트에선 직접."""
    u = db.query(User).filter(User.id == user_id).first()
    u.is_admin = True
    db.add(u)
    db.commit()


def _login(client, username: str) -> str:
    r = client.post("/auth/login", json={"username": username, "password": "pw-123456"})
    assert r.status_code == 200, r.text
    return r.json()["token"]


def test_admin_guard_blocks_normal_user(client):
    """일반 사용자의 관리 API 접근은 403 — 존재조차 알려 주지 않는다."""
    u = _register(client, "plain")
    token = _login(client, "plain")
    r = client.get("/admin/overview", headers={"Authorization": f"Bearer {token}"})
    assert r.status_code == 403


def test_admin_overview_and_users(client):
    """관리자는 전체 현황(카운트·사용자 목록)을 볼 수 있다."""
    u = _register(client, "boss")
    _register(client, "mate")
    db = Session()
    try:
        _make_admin(db, u["user"]["id"])
    finally:
        db.close()
    token = _login(client, "boss")

    ov = client.get("/admin/overview", headers={"Authorization": f"Bearer {token}"})
    assert ov.status_code == 200
    body = ov.json()
    assert body["users"] == 2
    assert body["disabled_users"] == 0

    users = client.get("/admin/users", headers={"Authorization": f"Bearer {token}"})
    assert users.status_code == 200
    rows = users.json()
    assert len(rows) == 2
    names = {r["username"] for r in rows}
    assert names == {"boss", "mate"}
    boss = next(r for r in rows if r["username"] == "boss")
    assert boss["is_admin"] is True


def test_disable_user_revokes_sessions_and_blocks_login(client):
    """정지 → 세션 즉시 삭제·API 차단·재로그인 거부 → 해제 → 복구."""
    u = _register(client, "target")
    admin = _register(client, "boss")
    db = Session()
    try:
        _make_admin(db, admin["user"]["id"])
    finally:
        db.close()
    a_token = _login(client, "boss")
    t_token = _login(client, "target")

    # 정지
    r = client.post(
        f"/admin/users/{u['user']['id']}/status",
        json={"disabled": True},
        headers={"Authorization": f"Bearer {a_token}"},
    )
    assert r.status_code == 200
    assert r.json()["sessions_revoked"] >= 1

    # 정지된 계정의 기존 토큰은 더 이상 통하지 않는다
    r = client.get("/auth/me", headers={"Authorization": f"Bearer {t_token}"})
    assert r.status_code in (401, 403)
    # 재로그인도 거부 — 자격증명이 맞아도
    r = client.post("/auth/login", json={"username": "target", "password": "pw-123456"})
    assert r.status_code == 403

    # 해제하면 다시 로그인할 수 있다
    r = client.post(
        f"/admin/users/{u['user']['id']}/status",
        json={"disabled": False},
        headers={"Authorization": f"Bearer {a_token}"},
    )
    assert r.status_code == 200
    r = client.post("/auth/login", json={"username": "target", "password": "pw-123456"})
    assert r.status_code == 200


def test_cannot_disable_self(client):
    """자기 계정 정지 금지 — 유일한 관리자가 스스로 잠그는 사고를 막는다."""
    admin = _register(client, "boss")
    db = Session()
    try:
        _make_admin(db, admin["user"]["id"])
    finally:
        db.close()
    token = _login(client, "boss")
    r = client.post(
        f"/admin/users/{admin['user']['id']}/status",
        json={"disabled": True},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert r.status_code == 400


def test_force_logout(client):
    """강제 로그아웃 — 세션만 지우고 계정은 남는다."""
    u = _register(client, "target")
    admin = _register(client, "boss")
    db = Session()
    try:
        _make_admin(db, admin["user"]["id"])
    finally:
        db.close()
    a_token = _login(client, "boss")
    t_token = _login(client, "target")

    r = client.post(
        f"/admin/users/{u['user']['id']}/logout",
        headers={"Authorization": f"Bearer {a_token}"},
    )
    assert r.status_code == 200
    assert r.json()["sessions_revoked"] >= 1
    r = client.get("/auth/me", headers={"Authorization": f"Bearer {t_token}"})
    assert r.status_code in (401, 403)
    # 계정은 살아 있으므로 다시 로그인 가능
    assert client.post("/auth/login", json={"username": "target", "password": "pw-123456"}).status_code == 200


def test_remove_member_and_delete_project(client):
    """멤버 제거·프로젝트 삭제 — 프로젝트 통째 정리."""
    owner = _register(client, "boss")
    mate = _register(client, "mate")
    db = Session()
    try:
        _make_admin(db, owner["user"]["id"])
    finally:
        db.close()
    token = _login(client, "boss")

    # 소유자 기기로 프로젝트 생성 + 팀원 기기 가입
    dev_id, dev_token = make_device(Session(), "boss-device")
    mate_dev_id, mate_token = make_device(Session(), "mate-device")
    r = client.post(
        "/projects",
        json={"display_name": "관리 테스트"},
        headers={"Authorization": f"Bearer {dev_token}"},
    )
    assert r.status_code == 200, r.text
    project_id = r.json()["id"]
    join_code = r.json()["join_code"]
    r = client.post(
        "/projects/join",
        json={"join_code": join_code, "device_name": "mate-device"},
        headers={"Authorization": f"Bearer {mate_token}"},
    )
    assert r.status_code == 200, r.text

    # 프로젝트 목록에 멤버 2명이 보인다
    projects = client.get("/admin/projects", headers={"Authorization": f"Bearer {token}"})
    assert projects.status_code == 200
    proj = next(p for p in projects.json() if p["id"] == project_id)
    assert proj["member_count"] == 2
    assert any(m["device_id"] == mate_dev_id for m in proj["members"])

    # 소유자는 제거할 수 없다
    r = client.delete(
        f"/admin/projects/{project_id}/members/{dev_id}",
        headers={"Authorization": f"Bearer {token}"},
    )
    assert r.status_code == 400

    # 팀원 제거
    r = client.delete(
        f"/admin/projects/{project_id}/members/{mate_dev_id}",
        headers={"Authorization": f"Bearer {token}"},
    )
    assert r.status_code == 200

    # 프로젝트 삭제 — 멤버·이벤트와 함께 사라진다
    r = client.delete(
        f"/admin/projects/{project_id}",
        headers={"Authorization": f"Bearer {token}"},
    )
    assert r.status_code == 200
    projects = client.get("/admin/projects", headers={"Authorization": f"Bearer {token}"})
    assert all(p["id"] != project_id for p in projects.json())


def test_events_feed(client):
    """활동 피드 — 이벤트가 있으면 kind·저장소와 함께 시간순으로."""
    admin = _register(client, "boss")
    db = Session()
    try:
        _make_admin(db, admin["user"]["id"])
    finally:
        db.close()
    token = _login(client, "boss")
    r = client.get("/admin/events?limit=10", headers={"Authorization": f"Bearer {token}"})
    assert r.status_code == 200
    assert isinstance(r.json(), list)
