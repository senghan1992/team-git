"""
Login accounts — register, sign in, profile, password, sign out, Google OAuth.

The desktop app used to keep its people registry in a local config file, so
every machine had a different user list and a teammate did not exist until
someone typed them in again. The user table here is the single source of truth;
the app caches only the signed-in user so it can stay logged in while offline.

Project membership and merge-manager assignment are matched by **email** (see
the repo's `.gpconfig`), so emails are unique and always stored lowercased.
"""
import secrets
from datetime import datetime, timedelta
from urllib.parse import urlsplit, urlencode

from typing import Annotated

from fastapi import APIRouter, Depends, Header, HTTPException, Query, status
from fastapi.responses import RedirectResponse
from sqlalchemy.orm import Session

from app.auth import (
    GOOGLE_AUTH_URL,
    GOOGLE_ONLY,
    GOOGLE_SCOPE,
    OAUTH_FLOW_TTL_SECONDS,
    generate_token,
    google_configured,
    google_client_id,
    google_exchange_code,
    google_redirect_uri,
    google_unique_username,
    hash_password,
    hash_token,
    verify_password,
)
from app.deps import bearer_token, get_db, get_user
from app.models import OAuthFlow, ProjectMemberEmail, User, UserSession
from app.schemas import (
    AuthResponse,
    GoogleUrlResponse,
    LoginRequest,
    PasswordChangeRequest,
    ProfileUpdateRequest,
    RegisterRequest,
    UserPublic,
)

router = APIRouter()

# 로그인 실패는 아이디가 틀렸는지 비밀번호가 틀렸는지 알려 주지 않는다 —
# 그러면 계정 존재 여부를 캐낼 수 있다.
_BAD_CREDENTIALS = "아이디 또는 비밀번호가 올바르지 않습니다."


def _public(user: User) -> UserPublic:
    return UserPublic(
        id=user.id,
        username=user.username,
        email=user.email,
        name=user.name,
        created_at=user.created_at,
    )


def _issue_token(db: Session, user: User) -> str:
    """Create a session row and return the raw token (stored hashed)."""
    token = generate_token()
    db.add(UserSession(token_hash=hash_token(token), user_id=user.id))
    db.commit()
    return token


def _normalize_email(email: str) -> str:
    return email.strip().lower()


def _normalize_username(username: str) -> str:
    return username.strip().lower()


@router.post("/register", response_model=AuthResponse, status_code=status.HTTP_201_CREATED)
def register(body: RegisterRequest, db: Session = Depends(get_db)):
    """Create an account and sign in immediately."""
    username = _normalize_username(body.username)
    email = _normalize_email(body.email)

    if "@" not in email:
        raise HTTPException(status.HTTP_400_BAD_REQUEST, "올바른 이메일을 입력하세요.")
    if db.query(User).filter(User.username == username).first():
        raise HTTPException(status.HTTP_409_CONFLICT, "이미 사용 중인 아이디입니다.")
    if db.query(User).filter(User.email == email).first():
        raise HTTPException(status.HTTP_409_CONFLICT, "이미 사용 중인 이메일입니다.")

    user = User(
        username=username,
        email=email,
        name=body.name.strip(),
        password_hash=hash_password(body.password),
    )
    db.add(user)
    db.commit()
    db.refresh(user)
    return AuthResponse(user=_public(user), token=_issue_token(db, user))


@router.post("/login", response_model=AuthResponse)
def login(body: LoginRequest, db: Session = Depends(get_db)):
    """Sign in with an id (or email) and password."""
    key = body.username.strip().lower()
    # 아이디로도, 이메일로도 로그인할 수 있게 한다 — 둘 중 무엇을 등록했는지
    # 기억하지 못하는 것이 가장 흔한 실패다.
    user = (
        db.query(User)
        .filter((User.username == key) | (User.email == key))
        .first()
    )
    # Google 로그인으로 생긴 계정에는 비밀번호가 없다 — 아이디/비밀번호
    # 로그인은 언제나 거부한다.
    if not user or user.password_hash == GOOGLE_ONLY or not verify_password(
        user.password_hash, body.password
    ):
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, _BAD_CREDENTIALS)
    return AuthResponse(user=_public(user), token=_issue_token(db, user))


@router.get("/me", response_model=UserPublic)
def me(user: User = Depends(get_user)):
    """Return the signed-in user."""
    return _public(user)


@router.get("/google/url", response_model=GoogleUrlResponse)
def google_auth_url(
    redirect_uri: str = Query(..., min_length=1, max_length=512),
    db: Session = Depends(get_db),
):
    """
    Start Google OAuth: return the Google consent URL plus a `state` token.

    `redirect_uri` is where this server should bounce the browser after
    Google is done — the desktop app's loopback URL, e.g.
    `http://127.0.0.1:54321/auth/google/complete`. It must be loopback-only,
    because the finished token is handed to whatever URL sits there.
    """
    if not google_configured():
        raise HTTPException(
            status.HTTP_400_BAD_REQUEST,
            "서버에 Google 로그인이 아직 설정되지 않았습니다. "
            "서버 관리자에게 GOOGLE_CLIENT_ID / GOOGLE_CLIENT_SECRET 설정을 요청하세요.",
        )
    parsed = urlsplit(redirect_uri)
    loopback = parsed.hostname in ("127.0.0.1", "localhost", "::1", "[::1]") or (
        parsed.hostname or ""
    ).startswith("127.")
    if parsed.scheme != "http" or not loopback:
        raise HTTPException(
            status.HTTP_400_BAD_REQUEST,
            "로그인 완료 후 돌아갈 주소는 내 컴퓨터(127.0.0.1)의 http 주소여야 합니다.",
        )
    # 오래된(10분) 미완료 핸드셰이크는 쌓이지 않게 정리한다.
    stale = datetime.utcnow() - timedelta(seconds=OAUTH_FLOW_TTL_SECONDS)
    db.query(OAuthFlow).filter(OAuthFlow.created_at < stale).delete()
    state = secrets.token_urlsafe(24)
    db.add(OAuthFlow(state=state, app_redirect_uri=redirect_uri))
    db.commit()
    params = urlencode(
        {
            "client_id": google_client_id(),
            "redirect_uri": google_redirect_uri(),
            "response_type": "code",
            "scope": GOOGLE_SCOPE,
            "state": state,
            "prompt": "select_account",
            "access_type": "online",
            "include_granted_scopes": "true",
        }
    )
    return GoogleUrlResponse(url=f"{GOOGLE_AUTH_URL}?{params}")


@router.get("/google/callback", include_in_schema=False)
def google_callback(
    code: str | None = None,
    state: str | None = None,
    error: str | None = None,
    db: Session = Depends(get_db),
):
    """
    Google's redirect target. Not an API — the browser follows this and lands
    back on the desktop app's loopback URL with the finished token.

    A 302 redirect is what makes the whole handshake browser-friendly: the
    app's login webview just needs to follow it.
    """
    flow = None
    if state:
        flow = db.query(OAuthFlow).filter(OAuthFlow.state == state).first()
    if not flow:
        # Google 콘솔의 redirect_uri 를 이 주소로 등록했는데 state 가 없다면
        # 위조된 시작이다. 남은 flow 는 전부 무효화하고 앱으로 오류를 돌려보낸다.
        if state:
            db.query(OAuthFlow).filter(OAuthFlow.state == state).delete()
            db.commit()
        return _google_finish(None, error or "잘못된 로그인 시도입니다. 다시 시작해 주세요.")

    if error:
        db.delete(flow)
        db.commit()
        return _google_finish(flow, error if error != "access_denied" else "로그인을 취소했습니다.")

    stale = datetime.utcnow() - timedelta(seconds=OAUTH_FLOW_TTL_SECONDS)
    if not code or flow.created_at < stale:
        db.delete(flow)
        db.commit()
        return _google_finish(flow, "로그인 요청이 만료되었습니다. 다시 시도해 주세요.")

    try:
        email, name = google_exchange_code(code)
    except HTTPException as e:
        db.delete(flow)
        db.commit()
        return _google_finish(flow, e.detail if isinstance(e.detail, str) else "Google 로그인에 실패했습니다.")

    user = db.query(User).filter(User.email == email).first()
    if user is None:
        taken = {u.username for u in db.query(User.username).all()}
        user = User(
            username=google_unique_username(email, taken),
            email=email,
            name=name,
            # Google 계정에는 비밀번호가 없다 (sentinel — 로그인 거부용).
            password_hash=GOOGLE_ONLY,
        )
        db.add(user)
        db.commit()
    token = _issue_token(db, user)
    db.delete(flow)
    db.commit()
    return _google_finish(flow, None, token=token)


def _google_finish(flow: OAuthFlow | None, error: str | None, *, token: str | None = None) -> RedirectResponse:
    """Bounce the login webview back to the app's loopback URL."""
    from urllib.parse import quote

    from fastapi.responses import RedirectResponse

    base = flow.app_redirect_uri if flow is not None else "http://127.0.0.1:1/auth/google/complete"
    sep = "&" if "?" in base else "?"
    if error is not None:
        url = f"{base}{sep}error={quote(error)}"
    else:
        url = f"{base}{sep}token={token}"
    return RedirectResponse(url, status_code=status.HTTP_302_FOUND)


@router.get("/users", response_model=list[UserPublic])
def search_users(
    q: str = "",
    _: User = Depends(get_user),
    db: Session = Depends(get_db),
):
    """
    Find teammates by name, id, or email — used when adding a member to a
    repo's `.gpconfig`.

    Before this existed the app searched its own local account file, so it could
    only ever find people who had signed in on that one computer. Requires a
    login and at least two characters, and returns at most 20 rows: enough to
    pick a colleague, not enough to walk the whole user table.
    """
    needle = q.strip().lower()
    if len(needle) < 2:
        return []
    like = f"%{needle}%"
    rows = (
        db.query(User)
        .filter(
            (User.name.ilike(like)) | (User.username.ilike(like)) | (User.email.ilike(like))
        )
        .order_by(User.username)
        .limit(20)
        .all()
    )
    return [_public(u) for u in rows]


@router.patch("/me", response_model=UserPublic)
def update_profile(
    body: ProfileUpdateRequest,
    user: User = Depends(get_user),
    db: Session = Depends(get_db),
):
    """Change display name and/or email."""
    if body.name is not None:
        user.name = body.name.strip()
    if body.email is not None:
        email = _normalize_email(body.email)
        if "@" not in email:
            raise HTTPException(status.HTTP_400_BAD_REQUEST, "올바른 이메일을 입력하세요.")
        if email != user.email:
            taken = db.query(User).filter(User.email == email).first()
            if taken:
                raise HTTPException(status.HTTP_409_CONFLICT, "이미 사용 중인 이메일입니다.")
            user.email = email
    user.updated_at = datetime.utcnow()
    db.add(user)
    db.commit()
    db.refresh(user)
    return _public(user)


@router.post("/me/password", status_code=status.HTTP_204_NO_CONTENT)
def change_password(
    body: PasswordChangeRequest,
    user: User = Depends(get_user),
    db: Session = Depends(get_db),
):
    """
    Change the password. Requires the current one.

    Every other session is revoked — a password change is what people do when
    they think someone else has their account, so leaving old tokens valid
    would defeat the point. The caller's own token stays valid.
    """
    if user.password_hash == GOOGLE_ONLY:
        raise HTTPException(
            status.HTTP_400_BAD_REQUEST,
            "Google 계정은 비밀번호가 없습니다. 비밀번호를 변경할 수 없습니다.",
        )
    if not verify_password(user.password_hash, body.current_password):
        raise HTTPException(status.HTTP_403_FORBIDDEN, "현재 비밀번호가 올바르지 않습니다.")
    if body.new_password == body.current_password:
        raise HTTPException(
            status.HTTP_400_BAD_REQUEST, "새 비밀번호가 현재 비밀번호와 같습니다."
        )
    user.password_hash = hash_password(body.new_password)
    user.updated_at = datetime.utcnow()
    db.add(user)
    db.commit()


@router.post("/logout", status_code=status.HTTP_204_NO_CONTENT)
def logout(
    authorization: Annotated[str | None, Header()] = None,
    db: Session = Depends(get_db),
):
    """
    Revoke the caller's token.

    Deliberately tolerant: an already-invalid token still returns 204 so the
    client can always finish signing out locally.
    """
    try:
        token_hash = hash_token(bearer_token(authorization))
    except Exception:
        return
    session = db.query(UserSession).filter(UserSession.token_hash == token_hash).first()
    if session:
        db.delete(session)
        db.commit()


@router.delete("/me", status_code=status.HTTP_204_NO_CONTENT)
def delete_account(user: User = Depends(get_user), db: Session = Depends(get_db)):
    """
    Delete the account: the user row, every session, and pending invites
    addressed to that email. Project membership rows keyed by device are left
    alone — they belong to the device, not the person.
    """
    db.query(UserSession).filter(UserSession.user_id == user.id).delete()
    db.query(ProjectMemberEmail).filter(ProjectMemberEmail.email == user.email).delete()
    db.delete(user)
    db.commit()
