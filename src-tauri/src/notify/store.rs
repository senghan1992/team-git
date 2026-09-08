use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::config_store::inbox_db_path;
use crate::error::{AppError, AppResult};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TeamEventRow {
    pub id: String,
    pub project_id: String,
    pub sender_device_name: String,
    pub event_kind: String,
    pub repo_name: String,
    pub payload: String,
    pub received_at: DateTime<Utc>,
    pub read: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotificationRow {
    pub id: String,
    pub channel_id: Option<String>,
    pub channel_kind: String,
    pub event_kind: String,
    pub repo_name: String,
    pub payload: String,
    pub status_code: Option<u16>,
    pub error: Option<String>,
    pub sent_at: DateTime<Utc>,
    pub read: bool,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open() -> AppResult<Self> {
        let path = inbox_db_path()?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(&path)?;
        Self::init_schema(&conn)?;
        Self::prune_old(&conn)?;
        Ok(Self { conn })
    }

    fn init_schema(c: &Connection) -> AppResult<()> {
        c.execute_batch(
            "CREATE TABLE IF NOT EXISTS notifications (
                id TEXT PRIMARY KEY,
                channel_id TEXT,
                channel_kind TEXT NOT NULL,
                event_kind TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                payload TEXT NOT NULL,
                status_code INTEGER,
                error TEXT,
                sent_at TEXT NOT NULL,
                read INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_sent_at ON notifications(sent_at DESC);
            CREATE INDEX IF NOT EXISTS idx_read ON notifications(read);
            CREATE TABLE IF NOT EXISTS team_events (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                sender_device_name TEXT NOT NULL DEFAULT '',
                event_kind TEXT NOT NULL,
                repo_name TEXT NOT NULL,
                payload TEXT NOT NULL,
                received_at TEXT NOT NULL,
                read INTEGER NOT NULL DEFAULT 0
            );
            CREATE INDEX IF NOT EXISTS idx_team_read ON team_events(read);
            ",
        )?;
        Ok(())
    }

    /// Drop entries older than 90 days.
    fn prune_old(c: &Connection) -> AppResult<()> {
        let cutoff = Utc::now() - chrono::Duration::days(90);
        c.execute(
            "DELETE FROM notifications WHERE sent_at < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn insert(&self, row: &NotificationRow) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO notifications (
                id, channel_id, channel_kind, event_kind, repo_name, payload,
                status_code, error, sent_at, read
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
            params![
                row.id,
                row.channel_id,
                row.channel_kind,
                row.event_kind,
                row.repo_name,
                row.payload,
                row.status_code.map(|v| v as i64),
                row.error,
                row.sent_at.to_rfc3339(),
                row.read as i32,
            ],
        )?;
        Ok(())
    }

    pub fn list(&self, limit: u32, unread_only: bool) -> AppResult<Vec<NotificationRow>> {
        let (sql, args): (&str, Vec<rusqlite::types::Value>) = if unread_only {
            (
                "SELECT id, channel_id, channel_kind, event_kind, repo_name, payload, status_code, error, sent_at, read
                 FROM notifications WHERE read = 0 ORDER BY sent_at DESC LIMIT ?1",
                vec![rusqlite::types::Value::from(limit as i64)],
            )
        } else {
            (
                "SELECT id, channel_id, channel_kind, event_kind, repo_name, payload, status_code, error, sent_at, read
                 FROM notifications ORDER BY sent_at DESC LIMIT ?1",
                vec![rusqlite::types::Value::from(limit as i64)],
            )
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(NotificationRow {
                    id: r.get(0)?,
                    channel_id: r.get(1)?,
                    channel_kind: r.get(2)?,
                    event_kind: r.get(3)?,
                    repo_name: r.get(4)?,
                    payload: r.get(5)?,
                    status_code: r.get::<_, Option<i64>>(6)?.map(|v| v as u16),
                    error: r.get(7)?,
                    sent_at: {
                        let s: String = r.get(8)?;
                        DateTime::parse_from_rfc3339(&s)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    8,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .with_timezone(&Utc)
                    },
                    read: r.get::<_, i64>(9)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_read(&self, id: &str) -> AppResult<()> {
        let n = self.conn.execute(
            "UPDATE notifications SET read = 1 WHERE id = ?1",
            params![id],
        )?;
        if n == 0 {
            return Err(AppError::Db(format!("notification {id} not found")));
        }
        Ok(())
    }

    pub fn count_unread(&self) -> AppResult<u32> {
        let n: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM notifications WHERE read = 0",
            [],
            |r| r.get(0),
        )?;
        Ok(n as u32)
    }

    pub fn get(&self, id: &str) -> AppResult<Option<NotificationRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, channel_id, channel_kind, event_kind, repo_name, payload, status_code, error, sent_at, read
             FROM notifications WHERE id = ?1",
        )?;
        let mut rows = stmt.query(params![id])?;
        if let Some(r) = rows.next()? {
            Ok(Some(NotificationRow {
                id: r.get(0)?,
                channel_id: r.get(1)?,
                channel_kind: r.get(2)?,
                event_kind: r.get(3)?,
                repo_name: r.get(4)?,
                payload: r.get(5)?,
                status_code: r.get::<_, Option<i64>>(6)?.map(|v| v as u16),
                error: r.get(7)?,
                sent_at: {
                    let s: String = r.get(8)?;
                    DateTime::parse_from_rfc3339(&s)
                        .map_err(|e| AppError::Db(e.to_string()))?
                        .with_timezone(&Utc)
                },
                read: r.get::<_, i64>(9)? != 0,
            }))
        } else {
            Ok(None)
        }
    }

    pub fn insert_team_event(&self, row: &TeamEventRow) -> AppResult<()> {
        self.conn.execute(
            "INSERT INTO team_events (id, project_id, sender_device_name, event_kind, repo_name, payload, received_at, read)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                row.id,
                row.project_id,
                row.sender_device_name,
                row.event_kind,
                row.repo_name,
                row.payload,
                row.received_at.to_rfc3339(),
                row.read as i32,
            ],
        )?;
        // 같은 브랜치의 재push 는 항목을 늘리지 않는다 — 팀원이 브랜치에 여러
        // 번 push 하면 최신 push 에 이전 push 가 전부 포함되므로 알림도
        // 하나(최신)로 합쳐야 수신함·배지가 "같은 브랜치가 여러 개"로 보이지
        // 않는다. 이전 push 알림은 읽음으로 대체한다 (최신 건이 그 브랜치의
        // 할 일을 대표한다).
        if row.event_kind.ends_with("branch_push") && !row.read {
            if let Some((url_key, branch)) = branch_key_of(&row.payload) {
                self.collapse_branch_push(&row.project_id, &url_key, &branch, &row.id)?;
            }
        }
        Ok(())
    }

    /// 같은 저장소·같은 브랜치의 이전 미읽음 branch_push 를 읽음으로 대체한다.
    fn collapse_branch_push(
        &self,
        project_id: &str,
        url_key: &str,
        branch: &str,
        except_id: &str,
    ) -> AppResult<()> {
        for r in self.list_team_events(10_000, true)? {
            if r.id == except_id || r.project_id != project_id {
                continue;
            }
            if !r.event_kind.ends_with("branch_push") {
                continue;
            }
            let Some((u, b)) = branch_key_of(&r.payload) else { continue };
            if u == url_key && b == branch {
                self.conn
                    .execute("UPDATE team_events SET read = 1 WHERE id = ?1", params![r.id])?;
            }
        }
        Ok(())
    }

    /// 병합이 끝난 브랜치의 남은 "병합 요청" 알림을 읽음 처리한다.
    ///
    /// 관리자가 병합 센터에서 바로 병합한 경우(토스트·수신함 버튼을 거치지
    /// 않음) 수신함에 "병합 요청" 카드가 남는다 — 이미 병합한 항목에 병합이
    /// 또 남아 보이는 꼴이다. 병합이 끝난 시점에 이 저장소·이 브랜치의
    /// 미읽음 branch_push 를 정리해 준다. 지운 건수를 돌려준다.
    pub fn mark_branch_push_read(&self, url_key: &str, branch: &str) -> AppResult<u32> {
        if url_key.is_empty() || branch.is_empty() {
            return Ok(0);
        }
        let mut n = 0u32;
        for r in self.list_team_events(10_000, true)? {
            if !r.event_kind.ends_with("branch_push") {
                continue;
            }
            let Some((u, b)) = branch_key_of(&r.payload) else { continue };
            if u == url_key && b == branch {
                self.conn
                    .execute("UPDATE team_events SET read = 1 WHERE id = ?1", params![r.id])?;
                n += 1;
            }
        }
        Ok(n)
    }

    pub fn list_team_events(&self, limit: u32, unread_only: bool) -> AppResult<Vec<TeamEventRow>> {
        let (sql, args): (&str, Vec<rusqlite::types::Value>) = if unread_only {
            (
                "SELECT id, project_id, sender_device_name, event_kind, repo_name, payload, received_at, read
                 FROM team_events WHERE read = 0 ORDER BY received_at DESC LIMIT ?1",
                vec![rusqlite::types::Value::from(limit as i64)],
            )
        } else {
            (
                "SELECT id, project_id, sender_device_name, event_kind, repo_name, payload, received_at, read
                 FROM team_events ORDER BY received_at DESC LIMIT ?1",
                vec![rusqlite::types::Value::from(limit as i64)],
            )
        };
        let mut stmt = self.conn.prepare(sql)?;
        let rows = stmt
            .query_map(rusqlite::params_from_iter(args.iter()), |r| {
                Ok(TeamEventRow {
                    id: r.get(0)?,
                    project_id: r.get(1)?,
                    sender_device_name: r.get(2)?,
                    event_kind: r.get(3)?,
                    repo_name: r.get(4)?,
                    payload: r.get(5)?,
                    received_at: {
                        let s: String = r.get(6)?;
                        DateTime::parse_from_rfc3339(&s)
                            .map_err(|e| {
                                rusqlite::Error::FromSqlConversionFailure(
                                    7,
                                    rusqlite::types::Type::Text,
                                    Box::new(e),
                                )
                            })?
                            .with_timezone(&Utc)
                    },
                    read: r.get::<_, i64>(7)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn mark_team_read(&self, id: &str) -> AppResult<()> {
        let n = self
            .conn
            .execute("UPDATE team_events SET read = 1 WHERE id = ?1", params![id])?;
        if n == 0 {
            return Err(AppError::Db(format!("team event {id} not found")));
        }
        Ok(())
    }

    /// 수신함의 "모두 읽음" — 지워진 개수를 돌려준다.
    pub fn mark_all_team_read(&self) -> AppResult<u32> {
        let n = self
            .conn
            .execute("UPDATE team_events SET read = 1 WHERE read = 0", [])?;
        Ok(n as u32)
    }

    pub fn count_unread_team_events(&self) -> AppResult<u32> {
        let n: i64 =
            self.conn
                .query_row("SELECT COUNT(*) FROM team_events WHERE read = 0", [], |r| {
                    r.get(0)
                })?;
        Ok(n as u32)
    }
}

#[allow(dead_code)]
pub fn new_id() -> String {
    Uuid::new_v4().to_string()
}

/// payload 에서 (정규화된 원격 URL, 브랜치) 짝을 뽑는다 — branch_push 를
/// 같은 브랜치끼리 합칠 때의 판별 열쇠. URL 이나 브랜치가 없으면(구버전
/// 서버·테스트용 payload) None — 합치지 않는다.
fn branch_key_of(payload: &str) -> Option<(String, String)> {
    if payload.is_empty() {
        return None;
    }
    let v: serde_json::Value = serde_json::from_str(payload).ok()?;
    let data = v.get("data")?;
    let url = data.get("url")?.as_str()?;
    let url_key = crate::git::normalize_remote_url(url);
    if url_key.is_empty() {
        return None;
    }
    let branch = data.get("branch")?.as_str()?.trim().to_string();
    if branch.is_empty() {
        return None;
    }
    Some((url_key, branch))
}
