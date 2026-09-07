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


def google_configured() -> bool:
    """True when the server operator set up all three Google env vars."""
    return bool(google_client_id()) and bool(os.environ.get("GOOGLE_CLIENT_SECRET", "").strip())


def google_redirect_uri() -> str:
    """The backend's own `/auth/google/callback` URL Google must redirect to."""
    return os.environ.get("GOOGLE_REDIRECT_URI", "").strip() or \
        f"http://127.0.0.1:{os.environ.get('PORT', '8000')}/auth/google/callback"


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
