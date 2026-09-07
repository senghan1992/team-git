"""Bearer-token authentication helpers."""
import hashlib
import os
import re
import secrets

import httpx
from fastapi import HTTPException, status


# Google 로그인으로 생긴 계정은 비밀번호가 없다. users.password_hash 는 NOT NULL
# (구버전 DB 는 ALTER TABLE 이 안 되고, 이 컬럼을 null 로 바꾸려면 테이블을
# 새로 만들어야 한다) 이므로 sentinel 값으로 표시한다.
GOOGLE_ONLY = "!google-only"


def generate_token() -> str:
    """Generate a random 32-byte URL-safe token."""
    return secrets.token_urlsafe(32)


def hash_token(token: str) -> str:
    """SHA-256 hash of a bearer token (hex encoded)."""
    return hashlib.sha256(token.encode()).hexdigest()


# ── Google OAuth ────────────────────────────────────────────────────────────
#
# 계정은 여전히 이 서버의 users 테이블이 소유한다. Google은 단지 "그 사람이
# 이 이메일 주소를 가졌다"를 증명하는 데 쓸 뿐이고, 로그인 토큰은 일반
# 로그인과 같은 방식으로 발급·저장한다.
#
# 서버 운영자는 Google Cloud Console 에서 OAuth client 를 만들고
# `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` / `GOOGLE_REDIRECT_URI` 세
# 환경변수를 설정한다. GOOGLE_REDIRECT_URI 는 콘솔에 등록한 것과 반드시
# 같아야 하는 이 서버의 `/auth/google/callback` 주소다 (예:
# http://127.0.0.1:8000/auth/google/callback).

GOOGLE_AUTH_URL = "https://accounts.google.com/o/oauth2/v2/auth"
GOOGLE_TOKEN_URL = "https://oauth2.googleapis.com/token"
GOOGLE_USERINFO_URL = "https://www.googleapis.com/oauth2/v3/userinfo"
GOOGLE_SCOPE = "openid email profile"
# 핸드셰이크(consent → callback)는 10분 안에 끝나야 한다.
OAUTH_FLOW_TTL_SECONDS = 600


def google_client_id() -> str:
    return os.environ.get("GOOGLE_CLIENT_ID", "").strip()


def auth_mode() -> str:
    """
    로그인 방식 — `AUTH_MODE` 환경변수로 바꾼다.

    - `simple` (기본): 예전 그대로 아이디+비밀번호 로그인/회원가입. 구글
      버튼은 UI 에서 감춰지고 Google 엔드포인트도 닫힌다.
    - `google`: Google 로그인 버튼이 켜진다 (아이디/비밀번호 로그인도 그대로
      동작 — 이미 만든 계정이 남아 있는 상태에서 잠기지 않게).

    사내 테스트처럼 "당분간은 예전 방식, 나중에 Google" 을 바꿔 가며 쓰기
    위한 스위치다. 오타 등은 관대히 받아들여 구글 계열 문자열이면 google 로
    본다 ("google auth" 포함).
    """
    mode = os.environ.get("AUTH_MODE", "").strip().lower()
    return "google" if mode in ("google", "google auth", "google-auth") else "simple"


def google_enabled() -> bool:
    """google 모드 + Google 설정 3종이 모두 준비됐을 때만 켜진다."""
    return auth_mode() == "google" and google_configured()


def google_configured() -> bool:
    """True when the server operator set up all three Google env vars."""
    return (
        bool(google_client_id())
        and bool(os.environ.get("GOOGLE_CLIENT_SECRET", "").strip())
        and bool(os.environ.get("GOOGLE_REDIRECT_URI", "").strip())
    )


def google_redirect_uri() -> str:
    """
    The backend's own `/auth/google/callback` URL Google must redirect to.

    필수 환경변수다 — Google Cloud Console 에 등록한 것과 **정확히** 같은 값을
    넣어야 한다. 예전에는 안 넣으면 127.0.0.1 로 임의 추정했는데, 서버가
    원격(팀 서버 IP)에 있으면 Google 이 사용자 컴퓨터의 127.0.0.1 로 돌려보내
    로그인이 영영 안 끝나는 원인이 됐다. 추정하지 말고 운영자가 명시하게 한다.
    """
    uri = os.environ.get("GOOGLE_REDIRECT_URI", "").strip()
    if not uri:
        raise HTTPException(
            status.HTTP_400_BAD_REQUEST,
            "서버에 GOOGLE_REDIRECT_URI 가 설정되지 않았습니다. "
            "Google Cloud Console 의 OAuth 클라이언트에 등록한 리디렉션 URI "
            "(예: http://<서버주소>:8000/auth/google/callback) 를 환경변수로 "
            "넣고 서버를 다시 시작하세요.",
        )
    return uri


def google_unique_username(email: str, taken: set[str]) -> str:
    """
    Derive a unique username from a Google email, e.g. hong.gildong → hong.gildong.

    Same email → same account, so two people with the same local part (foo@a.com,
    foo@b.com) get foo, foo2, foo3… until one is free.
    """
    base = re.sub(r"[^a-z0-9._-]", "", email.split("@", 1)[0].strip().lower())[:32] or "user"
    candidate, n = base, 2
    while candidate in taken:
        suffix = str(n)
        candidate = base[: 32 - len(suffix)] + suffix
        n += 1
    return candidate


def google_exchange_code(code: str) -> tuple[str, str]:
    """
    Exchange the one-time authorization code for the user's Google profile.

    Returns (email, name). Network errors and Google errors both raise
    HTTPException so the caller can abort the handshake.
    """
    client_id = google_client_id()
    client_secret = os.environ.get("GOOGLE_CLIENT_SECRET", "").strip()
    # 코드 교환에 쓰는 redirect_uri 는 동의 화면에 썼던 값과 반드시 같아야
    # 한다. 설정이 빠졌다면 깔끔한 오류를 돌려준다 (try 밖에서 판정).
    redirect_uri = google_redirect_uri()
    try:
        with httpx.Client(timeout=15) as client:
            resp = client.post(
                GOOGLE_TOKEN_URL,
                data={
                    "code": code,
                    "client_id": client_id,
                    "client_secret": client_secret,
                    "redirect_uri": google_redirect_uri(),
                    "grant_type": "authorization_code",
                },
            )
            resp.raise_for_status()
            access_token = resp.json()["access_token"]
            info = client.get(
                GOOGLE_USERINFO_URL,
                headers={"Authorization": f"Bearer {access_token}"},
            )
            info.raise_for_status()
            payload = info.json()
    except Exception as e:
        raise HTTPException(
            status.HTTP_502_BAD_GATEWAY,
            f"Google 인증 서버와 통신하지 못했습니다: {e}",
        )
    email = (payload.get("email") or "").strip().lower()
    if not email:
        raise HTTPException(status.HTTP_400_BAD_REQUEST, "Google 계정 이메일을 받지 못했습니다.")
    return email, (payload.get("name") or "").strip() or email.split("@", 1)[0]


def constant_time_compare(a: str, b: str) -> bool:
    """Constant-time string comparison to prevent timing attacks."""
    return secrets.compare_digest(a.encode(), b.encode())


def verify_token(expected_hash: str, provided: str) -> bool:
    """Verify a bearer token against its stored hash."""
    return constant_time_compare(expected_hash, hash_token(provided))


# ── Password hashing ─────────────────────────────────────────────────────────
#
# PBKDF2-HMAC-SHA256 from the standard library — no extra dependency, and
# unlike a bare SHA-256 it is salted and slow, so a stolen database cannot be
# reversed with a rainbow table. Format:
#
#     pbkdf2_sha256$<iterations>$<salt hex>$<hash hex>
#
# The iteration count is stored per row so it can be raised later without
# invalidating existing passwords.

PBKDF2_ITERATIONS = 210_000
_PBKDF2_PREFIX = "pbkdf2_sha256"


def hash_password(password: str, *, iterations: int = PBKDF2_ITERATIONS) -> str:
    """Hash a password for storage. Never store or log the plaintext."""
    salt = secrets.token_bytes(16)
    digest = hashlib.pbkdf2_hmac("sha256", password.encode(), salt, iterations)
    return f"{_PBKDF2_PREFIX}${iterations}${salt.hex()}${digest.hex()}"


def verify_password(stored: str, provided: str) -> bool:
    """
    Check a password against a stored hash.

    Returns False for anything unparseable rather than raising, so a corrupted
    or legacy row fails the login instead of 500-ing the endpoint.
    """
    try:
        scheme, iter_s, salt_hex, digest_hex = stored.split("$", 3)
        if scheme != _PBKDF2_PREFIX:
            return False
        digest = hashlib.pbkdf2_hmac(
            "sha256", provided.encode(), bytes.fromhex(salt_hex), int(iter_s)
        )
    except (ValueError, TypeError):
        return False
    return secrets.compare_digest(digest.hex(), digest_hex)


class AuthError(HTTPException):
    """401 authentication failure."""
    def __init__(self, detail: str = "Invalid or missing token"):
        super().__init__(status_code=status.HTTP_401_UNAUTHORIZED, detail=detail)
