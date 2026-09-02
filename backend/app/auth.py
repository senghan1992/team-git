"""Bearer-token authentication helpers."""
import hashlib
import secrets

from fastapi import HTTPException, status


def generate_token() -> str:
    """Generate a random 32-byte URL-safe token."""
    return secrets.token_urlsafe(32)


def hash_token(token: str) -> str:
    """SHA-256 hash of a bearer token (hex encoded)."""
    return hashlib.sha256(token.encode()).hexdigest()


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
