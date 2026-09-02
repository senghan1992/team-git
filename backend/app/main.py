"""FastAPI application entry point."""
from contextlib import asynccontextmanager

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware

from app.db import engine, Base
from app.routes import auth, devices, projects, members, events


@asynccontextmanager
async def lifespan(app: FastAPI):
    """Create all tables on startup; no-op on shutdown."""
    Base.metadata.create_all(bind=engine)
    yield


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


@app.get("/healthz")
async def healthz():
    return {"status": "ok"}
