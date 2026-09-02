"""SQLAlchemy engine and session factory."""
import os

from sqlalchemy import create_engine
from sqlalchemy.orm import sessionmaker, DeclarativeBase

DEFAULT_DB = os.environ.get(
    "GC_PEER_DB_URL", "sqlite+pysqlite:///./gc_peer.db"
)

engine = create_engine(DEFAULT_DB, connect_args={"check_same_thread": False})
Session = sessionmaker(bind=engine)


class Base(DeclarativeBase):
    """Base class for all SQLAlchemy models."""
    pass
