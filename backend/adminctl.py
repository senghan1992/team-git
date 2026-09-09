#!/usr/bin/env python3
"""서버 운영자용 관리자 부트스트랩 CLI.

관리자 권한(users.is_admin)은 가입 순서로 정해지지 않는다 — 서버를 띄운
사람이 직접 임명한다. 서버가 실행 중인 것과 무관하게 DB 를 직접 고치므로,
첫 관리자를 만들 때 이 스크립트 하나면 충분하다.

    python adminctl.py list                     # 관리자 목록
    python adminctl.py grant minji@example.com  # 관리자 임명
    python adminctl.py revoke minji@example.com # 관리자 해제

DB 위치는 서버와 같은 규칙(GC_PEER_DB_URL 환경변수, 기본 ./gc_peer.db)을
따른다 — 서버와 같은 디렉터리에서 실행하세요.
"""
import os
import sys

from sqlalchemy import text

# app.db 를 쓰기 전에 환경변수가 반영되도록 먼저 읽는다 (app.db 와 같은 규칙).
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

from app.db import Session  # noqa: E402
from app.models import User  # noqa: E402


def main() -> int:
    args = sys.argv[1:]
    if not args or args[0] not in ("list", "grant", "revoke"):
        print(__doc__)
        return 2
    cmd = args[0]
    db = Session()
    try:
        if cmd == "list":
            admins = db.query(User).filter(User.is_admin.is_(True)).all()
            if not admins:
                print("관리자가 없습니다 — grant <email> 으로 임명하세요.")
            for u in admins:
                print(f"  {u.email}  ({u.name})")
            return 0
        if len(args) < 2:
            print("이메일을 입력하세요. 예: python adminctl.py grant me@example.com")
            return 2
        email = args[1].strip().lower()
        user = db.query(User).filter(User.email == email).first()
        if not user:
            print(f"그 이메일의 계정이 없습니다: {email}")
            return 1
        user.is_admin = cmd == "grant"
        db.add(user)
        db.commit()
        print(f"{'관리자로 임명했습니다' if user.is_admin else '관리자에서 해제했습니다'}: {email}")
        return 0
    finally:
        db.close()


if __name__ == "__main__":
    raise SystemExit(main())
