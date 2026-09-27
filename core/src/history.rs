/**
 * Chat history on SQLite (`history.db` in the comrade-agent home).
 * Separate from vector memory: sessions + message transcripts for the
 * sidebar, browsable and deletable. Vector memory keeps semantic recall;
 * history keeps the verbatim conversation log.
 */
use rusqlite::{params, Connection};
use serde::Serialize;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Session title: first user line, truncated.
pub fn title_for(text: &str) -> String {
    let first = text.lines().next().unwrap_or("").trim();
    let short: String = first.chars().take(60).collect();
    if short.is_empty() { "New chat".to_string() } else { short }
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatSession {
    pub id: String,
    pub title: String,
    pub created_at: i64,
    pub updated_at: i64,
    pub message_count: i64,
}

#[derive(Clone, Debug, Serialize)]
pub struct ChatMessageRow {
    pub id: i64,
    pub role: String,
    pub content: String,
    pub created_at: i64,
}

pub struct HistoryStore {
    conn: Connection,
}

impl HistoryStore {
    pub fn open(path: &Path) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory() -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS sessions(
                id TEXT PRIMARY KEY,
                title TEXT NOT NULL,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS messages(
                id INTEGER PRIMARY KEY,
                session_id TEXT NOT NULL REFERENCES sessions(id) ON DELETE CASCADE,
                role TEXT NOT NULL,
                content TEXT NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_messages_session ON messages(session_id);
            PRAGMA foreign_keys = ON;",
        )?;
        Ok(())
    }

    pub fn create_session(&self, title: &str) -> anyhow::Result<String> {
        let now = now_ms();
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let id = format!("ses-{now}-{n}");
        self.conn.execute(
            "INSERT INTO sessions(id, title, created_at, updated_at) VALUES(?1, ?2, ?3, ?3)",
            params![id, title, now],
        )?;
        Ok(id)
    }

    pub fn add_message(&self, session_id: &str, role: &str, content: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "INSERT INTO messages(session_id, role, content, created_at) VALUES(?1, ?2, ?3, ?4)",
            params![session_id, role, content, now_ms()],
        )?;
        Ok(())
    }

    pub fn touch(&self, session_id: &str) -> anyhow::Result<()> {
        self.conn.execute(
            "UPDATE sessions SET updated_at=?1 WHERE id=?2",
            params![now_ms(), session_id],
        )?;
        Ok(())
    }

    pub fn session_exists(&self, session_id: &str) -> bool {
        self.conn
            .query_row("SELECT 1 FROM sessions WHERE id=?1", params![session_id], |_| Ok(()))
            .is_ok()
    }

    pub fn list_sessions(&self, limit: usize) -> Vec<ChatSession> {
        let mut stmt = match self.conn.prepare(
            "SELECT s.id, s.title, s.created_at, s.updated_at, COUNT(m.id)
             FROM sessions s LEFT JOIN messages m ON m.session_id = s.id
             GROUP BY s.id ORDER BY s.updated_at DESC LIMIT ?1",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![limit as i64], |r| {
            Ok(ChatSession {
                id: r.get(0)?,
                title: r.get(1)?,
                created_at: r.get(2)?,
                updated_at: r.get(3)?,
                message_count: r.get(4)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn get_messages(&self, session_id: &str) -> Vec<ChatMessageRow> {
        let mut stmt = match self.conn.prepare(
            "SELECT id, role, content, created_at FROM messages WHERE session_id=?1 ORDER BY id ASC",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![session_id], |r| {
            Ok(ChatMessageRow { id: r.get(0)?, role: r.get(1)?, content: r.get(2)?, created_at: r.get(3)? })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn delete_session(&self, session_id: &str) -> bool {
        let messages = self
            .conn
            .execute("DELETE FROM messages WHERE session_id=?1", params![session_id])
            .unwrap_or(0);
        let sessions = self
            .conn
            .execute("DELETE FROM sessions WHERE id=?1", params![session_id])
            .unwrap_or(0);
        messages + sessions > 0
    }

    pub fn rename_session(&self, session_id: &str, title: &str) -> bool {
        self.conn
            .execute(
                "UPDATE sessions SET title=?1, updated_at=?2 WHERE id=?3",
                params![title, now_ms(), session_id],
            )
            .map(|n| n > 0)
            .unwrap_or(false)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sessions_store_and_list() {
        let store = HistoryStore::open_in_memory().unwrap();
        let id = store.create_session("hello world").unwrap();
        store.add_message(&id, "user", "hello").unwrap();
        store.add_message(&id, "comrade", "hi there").unwrap();
        store.touch(&id).unwrap();
        let sessions = store.list_sessions(10);
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].message_count, 2);
        let msgs = store.get_messages(&id);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0].role, "user");
        assert!(store.delete_session(&id));
        assert!(store.list_sessions(10).is_empty());
    }

    #[test]
    fn titles_truncate() {
        assert_eq!(title_for("hello"), "hello");
        assert_eq!(title_for(""), "New chat");
        assert!(title_for(&"x".repeat(200)).chars().count() <= 60);
    }
}
