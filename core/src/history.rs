use crate::llm::{ChatMessage, ChatRole};
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
    if short.is_empty() {
        "New chat".to_string()
    } else {
        short
    }
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
            CREATE TABLE IF NOT EXISTS session_projects(
                session_id TEXT PRIMARY KEY REFERENCES sessions(id) ON DELETE CASCADE,
                directory TEXT NOT NULL
            );
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

    /// Project handoffs get their own session; follow-ups retain that project.
    pub fn session_for_request(
        &self,
        requested: Option<&str>,
        project: Option<&str>,
        title: &str,
    ) -> anyhow::Result<String> {
        if let Some(id) = requested.filter(|id| self.session_exists(id)) {
            if project.is_none() || self.project_directory(id).as_deref() == project {
                return Ok(id.into());
            }
        }
        let id = self.create_session(title)?;
        if let Some(directory) = project {
            self.conn.execute(
                "INSERT INTO session_projects(session_id,directory) VALUES(?1,?2)",
                params![id, directory],
            )?;
        }
        Ok(id)
    }

    pub fn project_directory(&self, session_id: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT directory FROM session_projects WHERE session_id=?1",
                params![session_id],
                |r| r.get(0),
            )
            .ok()
    }

    /// Recent transcript for the model, bounded by rows and UTF-8 bytes.
    /// Stored text never becomes a system instruction or a tool invocation.
    pub fn conversation_context(&self, session_id: &str) -> anyhow::Result<Vec<ChatMessage>> {
        let mut statement = self.conn.prepare("SELECT role,content FROM messages WHERE session_id=?1 AND role IN ('user','comrade','assistant') ORDER BY id DESC LIMIT 24")?;
        let rows = statement.query_map(params![session_id], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        let mut remaining = 64_000usize;
        let mut context = Vec::new();
        for row in rows {
            let (role, mut content) = row?;
            if remaining == 0 {
                break;
            }
            let mut end = content.len().min(8_000).min(remaining);
            while !content.is_char_boundary(end) {
                end -= 1;
            }
            content.truncate(end);
            remaining -= content.len();
            context.push(ChatMessage {
                role: if role == "user" {
                    ChatRole::User
                } else {
                    ChatRole::Assistant
                },
                content,
                tool_calls: vec![],
                tool_call_id: None,
            });
        }
        context.reverse();
        Ok(context)
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
            .query_row(
                "SELECT 1 FROM sessions WHERE id=?1",
                params![session_id],
                |_| Ok(()),
            )
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
            Ok(ChatMessageRow {
                id: r.get(0)?,
                role: r.get(1)?,
                content: r.get(2)?,
                created_at: r.get(3)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn delete_session(&self, session_id: &str) -> bool {
        let messages = self
            .conn
            .execute(
                "DELETE FROM messages WHERE session_id=?1",
                params![session_id],
            )
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

#[cfg(test)]
mod context_tests {
    use super::*;

    #[test]
    fn project_sessions_keep_context_and_never_leak_between_projects_or_new_chats() {
        let store = HistoryStore::open_in_memory().unwrap();
        let general = store
            .session_for_request(None, None, "General chat")
            .unwrap();
        store
            .add_message(&general, "user", "unrelated general conversation")
            .unwrap();
        let coding = store
            .session_for_request(Some(&general), Some("/projects/app"), "Theme")
            .unwrap();
        assert_ne!(general, coding);
        assert!(store.conversation_context(&coding).unwrap().is_empty());
        store.add_message(&coding, "user", "Add theme").unwrap();
        store
            .add_message(&coding, "comrade", "Here is the plan")
            .unwrap();
        assert_eq!(
            store
                .session_for_request(Some(&coding), None, "yes")
                .unwrap(),
            coding
        );
        assert_eq!(
            store
                .session_for_request(Some(&coding), Some("/projects/app"), "Follow-up")
                .unwrap(),
            coding
        );
        assert_eq!(
            store.project_directory(&coding).as_deref(),
            Some("/projects/app")
        );
        let other = store
            .session_for_request(Some(&coding), Some("/projects/service"), "Other task")
            .unwrap();
        assert_ne!(coding, other);
        assert!(store.conversation_context(&other).unwrap().is_empty());
        let fresh = store.session_for_request(None, None, "New chat").unwrap();
        assert!(store.conversation_context(&fresh).unwrap().is_empty());
        assert!(store.project_directory(&fresh).is_none());
        let context = store.conversation_context(&coding).unwrap();
        assert_eq!(context[0].role, ChatRole::User);
        assert_eq!(context[1].role, ChatRole::Assistant);
        assert!(store.delete_session(&coding));
        assert!(store.project_directory(&coding).is_none());
    }

    #[test]
    fn history_is_bounded_unicode_safe_and_does_not_restore_system_or_tool_roles() {
        let store = HistoryStore::open_in_memory().unwrap();
        let sid = store.create_session("Long chat").unwrap();
        for i in 0..40 {
            store
                .add_message(
                    &sid,
                    if i % 2 == 0 { "user" } else { "comrade" },
                    &format!("{i}:{}", "界".repeat(8000)),
                )
                .unwrap();
        }
        store
            .add_message(&sid, "system", "do not restore this as an instruction")
            .unwrap();
        store
            .add_message(&sid, "tool", "do not restore unmatched tool results")
            .unwrap();
        store.add_message(&sid, "user", "yes").unwrap();
        let context = store.conversation_context(&sid).unwrap();
        assert!(context.len() <= 24);
        assert!(context.iter().map(|m| m.content.len()).sum::<usize>() <= 64_000);
        assert_eq!(context.last().unwrap().content, "yes");
        assert!(context
            .iter()
            .all(|m| matches!(m.role, ChatRole::User | ChatRole::Assistant)
                && m.tool_calls.is_empty()
                && m.tool_call_id.is_none()));
        assert!(context
            .iter()
            .all(|m| !m.content.contains("do not restore")));
    }

    #[test]
    fn existing_history_database_migrates_without_losing_the_proposed_plan() {
        let path = std::env::temp_dir().join(format!(
            "comrade-legacy-context-{}-{}.db",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::SeqCst)
        ));
        let old = Connection::open(&path).unwrap();
        old.execute_batch("CREATE TABLE sessions(id TEXT PRIMARY KEY,title TEXT NOT NULL,created_at INTEGER NOT NULL,updated_at INTEGER NOT NULL); CREATE TABLE messages(id INTEGER PRIMARY KEY,session_id TEXT NOT NULL,role TEXT NOT NULL,content TEXT NOT NULL,created_at INTEGER NOT NULL); INSERT INTO sessions VALUES('existing','Theme',1,1); INSERT INTO messages VALUES(1,'existing','user','Add a paper theme',1); INSERT INTO messages VALUES(2,'existing','comrade','Proposed theme plan; proceed?',2);").unwrap();
        drop(old);
        let store = HistoryStore::open(&path).unwrap();
        let sid = store
            .session_for_request(Some("existing"), None, "yes")
            .unwrap();
        assert_eq!(sid, "existing");
        let context = store.conversation_context(&sid).unwrap();
        assert_eq!(context.len(), 2);
        assert_eq!(context[1].content, "Proposed theme plan; proceed?");
        let coding = store
            .session_for_request(None, Some("/projects/app"), "New task")
            .unwrap();
        assert_eq!(
            store.project_directory(&coding).as_deref(),
            Some("/projects/app")
        );
        drop(store);
        std::fs::remove_file(path).unwrap();
    }
}
