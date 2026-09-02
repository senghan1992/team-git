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


class AuthError(HTTPException):
    """401 authentication failure."""
    def __init__(self, detail: str = "Invalid or missing token"):
        super().__init__(status_code=status.HTTP_401_UNAUTHORIZED, detail=detail)
