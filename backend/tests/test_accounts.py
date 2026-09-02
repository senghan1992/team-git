"""로그인 계정 — 가입 / 로그인 / 프로필 / 비밀번호 / 로그아웃 / 탈퇴."""
from fastapi.testclient import TestClient

from app.auth import hash_password, verify_password


def _register(client: TestClient, **over) -> dict:
    body = {
        "name": "홍길동",
        "email": "hong@example.com",
        "username": "hong",
        "password": "pw-longenough",
    }
    body.update(over)
    return client.post("/auth/register", json=body)


# ── 비밀번호 해싱 ─────────────────────────────────────────────────────────────


def test_password_hash_is_salted_and_verifiable():
    a = hash_password("same-password")
    b = hash_password("same-password")
    assert a != b, "같은 비밀번호도 매번 다른 해시여야 한다 (솔트)"
    assert a.startswith("pbkdf2_sha256$")
    assert verify_password(a, "same-password")
    assert not verify_password(a, "same-passwor")
    # 해석할 수 없는 값은 예외가 아니라 실패로 떨어져야 한다.
    assert not verify_password("garbage", "same-password")
    assert not verify_password("", "same-password")


def test_password_is_never_stored_in_plaintext(client: TestClient, db_session):
    _register(client, password="plaintext-secret")
    from app.models import User

    user = db_session.query(User).filter(User.username == "hong").first()
    assert user is not None
    assert "plaintext-secret" not in user.password_hash


# ── 가입 ──────────────────────────────────────────────────────────────────────


def test_register_returns_user_and_token(client: TestClient):
    r = _register(client)
    assert r.status_code == 201, r.text
    body = r.json()
    assert body["user"]["username"] == "hong"
    assert body["user"]["email"] == "hong@example.com"
    assert body["user"]["name"] == "홍길동"
    assert body["token"]
    # 응답에 비밀번호 관련 필드가 새어 나가면 안 된다.
    assert "password" not in str(body["user"]).lower()


def test_register_normalizes_username_and_email(client: TestClient):
    r = _register(client, username="  HONG  ", email=" HONG@Example.COM ")
    assert r.status_code == 201, r.text
    assert r.json()["user"]["username"] == "hong"
    assert r.json()["user"]["email"] == "hong@example.com"


def test_register_rejects_duplicate_username_and_email(client: TestClient):
    assert _register(client).status_code == 201
    dup_user = _register(client, email="other@example.com")
    assert dup_user.status_code == 409
    assert "아이디" in dup_user.json()["detail"]
    dup_mail = _register(client, username="other")
    assert dup_mail.status_code == 409
    assert "이메일" in dup_mail.json()["detail"]


def test_register_rejects_bad_email_and_short_password(client: TestClient):
    assert _register(client, email="not-an-email").status_code == 400
    # 8자 미만은 스키마에서 걸린다 (422).
    assert _register(client, password="short").status_code == 422


# ── 로그인 ────────────────────────────────────────────────────────────────────


def test_login_with_username_or_email(client: TestClient):
    _register(client)
    by_id = client.post("/auth/login", json={"username": "hong", "password": "pw-longenough"})
    assert by_id.status_code == 200, by_id.text
    by_mail = client.post(
        "/auth/login", json={"username": "HONG@example.com", "password": "pw-longenough"}
    )
    assert by_mail.status_code == 200, "이메일로도 로그인되어야 한다"
    assert by_mail.json()["user"]["id"] == by_id.json()["user"]["id"]


def test_login_failure_does_not_reveal_whether_the_account_exists(client: TestClient):
    _register(client)
    wrong_pw = client.post("/auth/login", json={"username": "hong", "password": "nope-nope"})
    no_such = client.post("/auth/login", json={"username": "ghost", "password": "nope-nope"})
    assert wrong_pw.status_code == no_such.status_code == 401
    assert wrong_pw.json()["detail"] == no_such.json()["detail"]


# ── 내 정보 ───────────────────────────────────────────────────────────────────


def test_me_requires_a_token(client: TestClient):
    assert client.get("/auth/me").status_code == 401
    assert client.get("/auth/me", headers={"Authorization": "Bearer nope"}).status_code == 401
    assert client.get("/auth/me", headers={"Authorization": "hong"}).status_code == 401


def test_me_returns_the_signed_in_user(client: TestClient):
    token = _register(client).json()["token"]
    r = client.get("/auth/me", headers={"Authorization": f"Bearer {token}"})
    assert r.status_code == 200
    assert r.json()["email"] == "hong@example.com"


def test_device_token_cannot_be_used_as_a_user_token(client: TestClient, auth_headers):
    """기기 토큰과 사람 토큰은 서로 통하지 않아야 한다."""
    assert client.get("/auth/me", headers=auth_headers).status_code == 401


# ── 프로필 수정 ───────────────────────────────────────────────────────────────


def test_update_profile_changes_name_and_email(client: TestClient):
    token = _register(client).json()["token"]
    h = {"Authorization": f"Bearer {token}"}
    r = client.patch("/auth/me", json={"name": "홍길순", "email": "SOON@Example.com"}, headers=h)
    assert r.status_code == 200, r.text
    assert r.json()["name"] == "홍길순"
    assert r.json()["email"] == "soon@example.com"


def test_update_profile_rejects_an_email_someone_else_has(client: TestClient):
    _register(client)
    other = _register(client, username="kim", email="kim@example.com").json()["token"]
    r = client.patch(
        "/auth/me",
        json={"email": "hong@example.com"},
        headers={"Authorization": f"Bearer {other}"},
    )
    assert r.status_code == 409


def test_update_profile_allows_resaving_my_own_email(client: TestClient):
    token = _register(client).json()["token"]
    r = client.patch(
        "/auth/me",
        json={"email": "hong@example.com"},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert r.status_code == 200, "자기 이메일을 그대로 저장하는 건 충돌이 아니다"


# ── 비밀번호 변경 ─────────────────────────────────────────────────────────────


def test_change_password_then_login_with_the_new_one(client: TestClient):
    token = _register(client).json()["token"]
    h = {"Authorization": f"Bearer {token}"}
    r = client.post(
        "/auth/me/password",
        json={"current_password": "pw-longenough", "new_password": "brand-new-pw"},
        headers=h,
    )
    assert r.status_code == 204, r.text
    assert client.post(
        "/auth/login", json={"username": "hong", "password": "pw-longenough"}
    ).status_code == 401, "옛 비밀번호는 더 이상 통하지 않아야 한다"
    assert client.post(
        "/auth/login", json={"username": "hong", "password": "brand-new-pw"}
    ).status_code == 200


def test_change_password_requires_the_current_one(client: TestClient):
    token = _register(client).json()["token"]
    r = client.post(
        "/auth/me/password",
        json={"current_password": "wrong-one", "new_password": "brand-new-pw"},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert r.status_code == 403


def test_change_password_rejects_reusing_the_same_password(client: TestClient):
    token = _register(client).json()["token"]
    r = client.post(
        "/auth/me/password",
        json={"current_password": "pw-longenough", "new_password": "pw-longenough"},
        headers={"Authorization": f"Bearer {token}"},
    )
    assert r.status_code == 400


# ── 로그아웃 / 탈퇴 ───────────────────────────────────────────────────────────


def test_logout_revokes_the_token(client: TestClient):
    token = _register(client).json()["token"]
    h = {"Authorization": f"Bearer {token}"}
    assert client.post("/auth/logout", headers=h).status_code == 204
    assert client.get("/auth/me", headers=h).status_code == 401, "토큰이 실제로 무효화돼야 한다"


def test_logout_is_tolerant_of_a_bad_token(client: TestClient):
    """클라이언트는 어떤 상황에서도 로그아웃을 끝낼 수 있어야 한다."""
    assert client.post("/auth/logout").status_code == 204
    assert client.post("/auth/logout", headers={"Authorization": "Bearer nope"}).status_code == 204


def test_logout_does_not_touch_other_sessions(client: TestClient):
    """다른 기기에서 로그인한 세션은 살아 있어야 한다."""
    _register(client)
    a = client.post("/auth/login", json={"username": "hong", "password": "pw-longenough"}).json()["token"]
    b = client.post("/auth/login", json={"username": "hong", "password": "pw-longenough"}).json()["token"]
    client.post("/auth/logout", headers={"Authorization": f"Bearer {a}"})
    assert client.get("/auth/me", headers={"Authorization": f"Bearer {b}"}).status_code == 200


def test_delete_account_removes_the_user_and_its_sessions(client: TestClient):
    token = _register(client).json()["token"]
    h = {"Authorization": f"Bearer {token}"}
    assert client.delete("/auth/me", headers=h).status_code == 204
    assert client.get("/auth/me", headers=h).status_code == 401
    assert client.post(
        "/auth/login", json={"username": "hong", "password": "pw-longenough"}
    ).status_code == 401
    # 같은 아이디로 다시 가입할 수 있어야 한다.
    assert _register(client).status_code == 201
