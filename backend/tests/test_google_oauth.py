"""Google OAuth 로그인 — 시작 / 콜백 / 계정 생성·연결 / 비밀번호 로그인 차단."""
import pytest
from fastapi.testclient import TestClient

from app import auth as auth_mod
from app.models import OAuthFlow, User


@pytest.fixture
def google_env(monkeypatch):
    """Google 로그인이 설정된 서버 상태."""
    monkeypatch.setenv("GOOGLE_CLIENT_ID", "test-client-id")
    monkeypatch.setenv("GOOGLE_CLIENT_SECRET", "test-client-secret")
    monkeypatch.setenv(
        "GOOGLE_REDIRECT_URI", "http://127.0.0.1:8000/auth/google/callback"
    )


def _fake_google(monkeypatch, email="hong@example.com", name="홍길동"):
    """구글과의 실제 통신 없이 콜백이 끝까지 흐르게 만든다."""
    # routes/auth.py 는 app.auth 의 함수를 이름으로 import 했으므로
    # 호출 시점에 보이는 모듈 네임스페이스 쪽을 바꿔야 한다.
    import app.routes.auth as routes_auth

    monkeypatch.setattr(routes_auth, "google_exchange_code", lambda code: (email, name))


def _start(client: TestClient) -> dict:
    """앱이 로그인을 시작했을 때 하는 요청 — (url, state) 를 돌려준다."""
    r = client.get(
        "/auth/google/url", params={"redirect_uri": "http://127.0.0.1:54321/auth/google/complete"}
    )
    assert r.status_code == 200, r.text
    body = r.json()
    assert body["url"].startswith("https://accounts.google.com/o/oauth2/v2/auth?")
    state = dict(x.split("=", 1) for x in body["url"].split("?", 1)[1].split("&"))["state"]
    return {"url": body["url"], "state": state}


# ── 시작 ──────────────────────────────────────────────────────────────────────


def test_url_requires_google_configuration(client: TestClient, monkeypatch):
    monkeypatch.delenv("GOOGLE_CLIENT_ID", raising=False)
    r = client.get("/auth/google/url", params={"redirect_uri": "http://127.0.0.1:1/x"})
    assert r.status_code == 400
    assert "설정" in r.json()["detail"]


def test_url_rejects_non_loopback_redirect(client: TestClient, google_env):
    r = client.get("/auth/google/url", params={"redirect_uri": "http://evil.example.com/steal"})
    assert r.status_code == 400
    assert "127.0.0.1" in r.json()["detail"]


def test_url_contains_client_id_and_backend_redirect(client: TestClient, google_env):
    r = client.get(
        "/auth/google/url", params={"redirect_uri": "http://127.0.0.1:54321/auth/google/complete"}
    )
    body = r.json()
    assert "client_id=test-client-id" in body["url"]
    assert "redirect_uri=http%3A%2F%2F127.0.0.1%3A8000%2Fauth%2Fgoogle%2Fcallback" in body["url"]
    # 한 번 시작하면 핸드셰이크 행이 하나 생긴다 (state 로 추적).
    assert "state=" in body["url"]


# ── 콜백 ──────────────────────────────────────────────────────────────────────


def test_callback_with_unknown_state_fails_safely():
    """state 를 모르는 요청은 같은 앱 주소로 오류를 돌려보낸다 — 열려 있는
    앱이든 닫혀 있는 앱이든, 성공처럼 보이면 안 된다."""
    from app.main import app

    with TestClient(app) as client:
        r = client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": "forged-state", "code": "x"}
        )
        assert r.status_code == 302
        assert "error=" in r.headers["location"]
        assert client.get("/auth/me").status_code == 401, "토큰 같은 게 발급되면 안 된다"


def test_callback_creates_an_account_and_signs_in(
    client: TestClient, google_env, monkeypatch, db_session
):
    _fake_google(monkeypatch)
    start = _start(client)
    r = client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": start["state"], "code": "any"},
    )
    assert r.status_code == 302, r.text
    location = r.headers["location"]
    assert location.startswith("http://127.0.0.1:54321/auth/google/complete?")
    token = dict(x.split("=", 1) for x in location.split("?", 1)[1].split("&"))["token"]

    me = client.get("/auth/me", headers={"Authorization": f"Bearer {token}"})
    assert me.status_code == 200
    assert me.json()["email"] == "hong@example.com"
    assert me.json()["name"] == "홍길동"
    assert me.json()["username"] == "hong"

    user = db_session.query(User).filter(User.email == "hong@example.com").first()
    assert user is not None
    assert user.password_hash == auth_mod.GOOGLE_ONLY

    # 핸드셰이크 행은 1회용 — 다시 쓰면 실패해야 한다.
    again = client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": start["state"], "code": "any"},
    )
    assert "error=" in again.headers["location"]


def test_callback_logs_into_an_existing_account(
    client: TestClient, google_env, monkeypatch, db_session
):
    """같은 이메일로 다시 온 로그인은 새 계정을 만들지 않는다."""
    _fake_google(monkeypatch)
    first = _start(client)
    client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": first["state"], "code": "any"},
    )
    users = db_session.query(User).filter(User.email == "hong@example.com").count()
    assert users == 1

    second = _start(client)
    r = client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": second["state"], "code": "any"},
    )
    assert "token=" in r.headers["location"]
    assert db_session.query(User).filter(User.email == "hong@example.com").count() == 1
    assert db_session.query(OAuthFlow).count() == 0


def test_google_account_cannot_log_in_with_password(client: TestClient, google_env, monkeypatch):
    _fake_google(monkeypatch)
    start = _start(client)
    client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": start["state"], "code": "any"},
    )
    r = client.post("/auth/login", json={"username": "hong@example.com", "password": "guess-pw"})
    assert r.status_code == 401
    assert "올바르지 않습니다" in r.json()["detail"]


def test_google_account_cannot_change_password(client: TestClient, google_env, monkeypatch):
    _fake_google(monkeypatch)
    start = _start(client)
    r = client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": start["state"], "code": "any"},
    )
    token = dict(x.split("=", 1) for x in r.headers["location"].split("?", 1)[1].split("&"))["token"]
    r = client.post(
        "/auth/me/password",
        json={"current_password": "whatever", "new_password": "brand-new-pw"},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert r.status_code == 400
    assert "Google 계정" in r.json()["detail"]


def test_callback_handles_user_cancellation(client: TestClient, google_env):
    start = _start(client)
    r = client.get(
        "/auth/google/callback", follow_redirects=False,
        params={"state": start["state"], "code": "x", "error": "access_denied"},
    )
    assert r.status_code == 302
    assert "error=" in r.headers["location"]


def test_username_collision_gets_a_suffix(client: TestClient, google_env, monkeypatch):
    """foo@a.com, foo@b.com 두 사람이 Google 로그인하면 foo, foo2 가 된다."""
    _fake_google(monkeypatch, email="hong@a.com")
    start = _start(client)
    client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": start["state"], "code": "1"},
    )

    _fake_google(monkeypatch, email="hong@b.com")
    start = _start(client)
    client.get(
        "/auth/google/callback", follow_redirects=False, params={"state": start["state"], "code": "1"},
    )

    from app.db import Session

    db = Session()
    names = sorted(u.username for u in db.query(User).filter(User.email.like("%@%")).all())
    assert "hong" in names and "hong2" in names
    db.close()
