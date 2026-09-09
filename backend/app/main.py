"""FastAPI application entry point."""
from contextlib import asynccontextmanager

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from app.db import engine, Base
from app.routes import admin, auth, devices, projects, members, events


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Create all tables on startup; no-op on shutdown."""
    _migrate_admin_columns()
    Base.metadata.create_all(bind=engine)
    yield


def _migrate_admin_columns():
    """기존 DB 에 관리자 컬럼을 붙인다.

    create_all 은 이미 있는 테이블을 건드리지 않으므로, 운영 중 서버를 새
    코드로 띄우면 users 테이블에 is_admin/disabled/last_login_at 이 없어
    첫 조회부터 500 이 났다. 컬럼 존재를 확인하고 ALTER TABLE ADD COLUMN —
    SQLite 의 ADD COLUMN 은 상수 기본값만 가능하지만 이 세 컬럼은 충분하다.
    """
    from sqlalchemy import text

    if engine.dialect.name != "sqlite":
        return
    with engine.begin() as conn:
        cols = {
            row[1]
            for row in conn.execute(text("PRAGMA table_info(users)"))
        }
        if not cols:
            return  # users 테이블이 아직 없다 — create_all 이 만들어 준다
        if "is_admin" not in cols:
            conn.execute(text("ALTER TABLE users ADD COLUMN is_admin BOOLEAN NOT NULL DEFAULT 0"))
        if "disabled" not in cols:
            conn.execute(text("ALTER TABLE users ADD COLUMN disabled BOOLEAN NOT NULL DEFAULT 0"))
        if "last_login_at" not in cols:
            conn.execute(text("ALTER TABLE users ADD COLUMN last_login_at DATETIME NULL"))


app = FastAPI(title="Git Companion Peer Backend", version="0.1.0", lifespan=lifespan)

app.add_middleware(
    CORSMiddleware,
    allow_origins=["*"],
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
)

app.include_router(auth.router, prefix="/auth", tags=["auth"])
app.include_router(devices.router, prefix="/devices", tags=["devices"])
app.include_router(projects.router, prefix="/projects", tags=["projects"])
app.include_router(members.router, prefix="/projects", tags=["members"])
app.include_router(events.router, prefix="/events", tags=["events"])
app.include_router(admin.router, prefix="/admin", tags=["admin"])


@app.get("/healthz")
async def healthz():
    return {"status": "ok"}
