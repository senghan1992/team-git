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
        Ok(())
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
