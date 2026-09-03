# Git Companion 팀 서버 (FastAPI) — 루트의 docker-compose.yml 이 이 파일을 빌드한다.
#
#   docker compose up -d          # 루트에서 — 서버가 8000 포트에 뜬다
#   docker compose logs -f        # 로그 보기
#
# 데이터(계정·팀·알림)는 SQLite 파일 하나(/data/gc_peer.db)에 저장되고, compose 가
# 이 경로를 호스트의 ./data 에 마운트하므로 컨테이너를 지워도 남는다.
FROM python:3.11-slim

ENV PYTHONDONTWRITEBYTECODE=1 \
    PYTHONUNBUFFERED=1 \
    PIP_NO_CACHE_DIR=1 \
    GC_PEER_DB_URL=sqlite+pysqlite:////data/gc_peer.db

WORKDIR /app

# 의존성 레이어를 먼저 만들어 소스만 바뀌었을 때 재설치를 피한다.
COPY pyproject.toml ./
COPY app ./app
RUN pip install . && mkdir -p /data

VOLUME ["/data"]
EXPOSE 8000

# 컨테이너 안에는 curl 이 없다 — 표준 라이브러리로 /healthz 를 확인한다.
HEALTHCHECK --interval=30s --timeout=3s --start-period=10s --retries=3 \
  CMD python -c "import urllib.request,sys; r=urllib.request.urlopen('http://127.0.0.1:8000/healthz', timeout=2); sys.exit(0 if r.status==200 else 1)"

CMD ["uvicorn", "app.main:app", "--host", "0.0.0.0", "--port", "8000"]
