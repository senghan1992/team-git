"""Project creation, join, and listing endpoints."""
import random

from fastapi import APIRouter, Depends, HTTPException, status
from sqlalchemy.orm import Session
from sqlalchemy.exc import IntegrityError

from app.deps import get_db, get_device
from app.models import Device, Project, ProjectMember
from app.schemas import (
    ProjectCreateRequest,
    ProjectCreateResponse,
    ProjectJoinRequest,
    ProjectInfo,
    ProjectListResponse,
)

router = APIRouter()

_JOIN_CHARS = "ABCDEFGHJKLMNPQRSTUVWXYZ23456789"


def _generate_join_code() -> str:
    """Generate an 8-char join code like K7H2-9XQA."""
    raw = "".join(random.choices(_JOIN_CHARS, k=8))
    return raw  # stored raw, no dash


@router.post("", response_model=ProjectCreateResponse)
def create_project(
    body: ProjectCreateRequest,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """Create a new project and register the caller as its owner."""
    join_code = _generate_join_code()

    # Retry on collision (very unlikely)
    for _ in range(10):
        existing = db.query(Project).filter(Project.join_code == join_code).first()
        if not existing:
            break
        join_code = _generate_join_code()
    else:
        raise HTTPException(status_code=500, detail="Failed to generate unique join code")

    project = Project(
        display_name=body.display_name,
        join_code=join_code,
        created_by=device.id,
    )
    db.add(project)
    db.flush()  # populate project.id before building ProjectMember

    member = ProjectMember(
        project_id=project.id,
        device_id=device.id,
        role="owner",
    )
    db.add(member)
    try:
        db.commit()
    except IntegrityError:
        db.rollback()
        raise HTTPException(status_code=409, detail="Join code collision; please retry")

    return ProjectCreateResponse(
        id=project.id,
        display_name=project.display_name,
        join_code=f"{project.join_code[:4]}-{project.join_code[4:]}",
        role="owner",
    )


@router.post("/join", response_model=ProjectCreateResponse)
def join_project(
    body: ProjectJoinRequest,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """Join an existing project using its join code."""
    raw_code = body.join_code.upper().replace("-", "")
    project = (
        db.query(Project)
        .filter(Project.join_code == raw_code)
        .first()
    )
    if not project:
        raise HTTPException(status_code=404, detail="Invalid join code")

    existing = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == project.id,
            ProjectMember.device_id == device.id,
        )
        .first()
    )
    if existing:
        raise HTTPException(status_code=409, detail="Already a member of this project")

    member = ProjectMember(
        project_id=project.id,
        device_id=device.id,
        role="member",
    )
    db.add(member)
    db.commit()

    return ProjectCreateResponse(
        id=project.id,
        display_name=project.display_name,
        join_code=f"{project.join_code[:4]}-{project.join_code[4:]}",
        role="member",
    )


@router.get("", response_model=ProjectListResponse)
def list_projects(
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """List all projects the authenticated device is a member of."""
    rows = (
        db.query(Project, ProjectMember)
        .join(ProjectMember, Project.id == ProjectMember.project_id)
        .filter(ProjectMember.device_id == device.id)
        .all()
    )
    projects = [
        ProjectInfo(
            id=project.id,
            display_name=project.display_name,
            join_code=f"{project.join_code[:4]}-{project.join_code[4:]}",
            role=member.role,
        )
        for project, member in rows
    ]
    return ProjectListResponse(projects=projects)


@router.get("/{project_id}", response_model=ProjectInfo)
def get_project(
    project_id: str,
    device: Device = Depends(get_device),
    db: Session = Depends(get_db),
):
    """Get a single project by ID (must be a member)."""
    member = (
        db.query(ProjectMember)
        .filter(
            ProjectMember.project_id == project_id,
            ProjectMember.device_id == device.id,
        )
        .first()
    )
    if not member:
        raise HTTPException(status_code=403, detail="Not a member of this project")

    project = db.query(Project).filter(Project.id == project_id).first()
    if not project:
        raise HTTPException(status_code=404, detail="Project not found")

    return ProjectInfo(
        id=project.id,
        display_name=project.display_name,
        join_code=f"{project.join_code[:4]}-{project.join_code[4:]}",
        role=member.role,
    )
