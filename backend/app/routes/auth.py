"""
Login accounts — register, sign in, profile, password, sign out.

The desktop app used to keep its people registry in a local config file, so
every machine had a different user list and a teammate did not exist until
someone typed them in again. The user table here is the single source of truth;
the app caches only the signed-in user so it can stay logged in while offline.

Project membership and merge-manager assignment are matched by **email** (see
the repo's `.gpconfig`), so emails are unique and always stored lowercased.
"""
from datetime import datetime

from typing import Annotated

from fastapi import APIRouter, Depends, Header, HTTPException, status
from sqlalchemy.orm import Session

from app.auth import (
    generate_token,
    hash_password,
    hash_token,
    verify_password,
)
from app.deps import bearer_token, get_db, get_user
from app.models import ProjectMemberEmail, User, UserSession
from app.schemas import (
    AuthResponse,
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
    if not user or not verify_password(user.password_hash, body.password):
        raise HTTPException(status.HTTP_401_UNAUTHORIZED, _BAD_CREDENTIALS)
    return AuthResponse(user=_public(user), token=_issue_token(db, user))


@router.get("/me", response_model=UserPublic)
def me(user: User = Depends(get_user)):
    """Return the signed-in user."""
    return _public(user)


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
