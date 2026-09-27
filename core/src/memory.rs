/**
 * Long-term memory on SQLite: facts, preferences, projects, task summaries
 * and ChatGPT imports, each with an embedding vector. Recall is cosine
 * similarity over vectors stored as BLOBs (brute force — personal-memory
 * scale is thousands of rows, single-digit ms). No native vector extension
 * needed; the layout stays compatible if one is adopted later.
 */
use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use crate::llm::Embedder;

/// Shareable handle: `Connection` is `Send` but not `Sync`, so a plain
/// `std` mutex (not a tokio mutex held across `.await`) guards it.
pub type SharedMemory = Arc<Mutex<MemoryStore>>;

#[derive(Clone, Debug, Serialize)]
pub struct ScoredMemory {
    pub id: i64,
    pub kind: String,
    pub text: String,
    pub source: String,
    pub sim: f32,
}

#[derive(Clone, Debug, Serialize)]
pub struct MemoryItem {
    pub id: i64,
    pub kind: String,
    pub text: String,
    pub source: String,
    pub created_at: i64,
}

pub struct MemoryStore {
    conn: Connection,
    dim: usize,
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

pub fn encode_vec(v: &[f32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(v.len() * 4);
    for f in v {
        out.extend_from_slice(&f.to_le_bytes());
    }
    out
}

pub fn decode_vec(bytes: &[u8]) -> Vec<f32> {
    let (chunks, _) = bytes.as_chunks::<4>();
    chunks.iter().map(|c| f32::from_le_bytes(*c)).collect()
}

pub fn cosine(a: &[f32], b: &[f32]) -> f32 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }
    let (mut dot, mut na, mut nb) = (0.0f32, 0.0f32, 0.0f32);
    for (x, y) in a.iter().zip(b.iter()) {
        dot += x * y;
        na += x * x;
        nb += y * y;
    }
    if na == 0.0 || nb == 0.0 {
        return 0.0;
    }
    dot / (na.sqrt() * nb.sqrt())
}

/// Greedy paragraph-aware chunking for long imports.
pub fn chunk_text(text: &str, max_chars: usize) -> Vec<String> {
    let mut chunks = Vec::new();
    let mut current = String::new();
    for para in text.split("\n\n") {
        let para = para.trim();
        if para.is_empty() {
            continue;
        }
        if current.len() + para.len() + 2 > max_chars && !current.is_empty() {
            chunks.push(current.trim().to_string());
            current = String::new();
        }
        if para.len() > max_chars {
            // Single huge paragraph: hard-split on char boundaries.
            let chars: Vec<char> = para.chars().collect();
            for piece in chars.chunks(max_chars) {
                chunks.push(piece.iter().collect());
            }
        } else {
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(para);
        }
    }
    if !current.trim().is_empty() {
        chunks.push(current.trim().to_string());
    }
    chunks
}

impl MemoryStore {
    pub fn open(path: &Path, dim: usize) -> anyhow::Result<Self> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let store = Self { conn, dim };
        store.init()?;
        Ok(store)
    }

    #[cfg(test)]
    pub fn open_in_memory(dim: usize) -> anyhow::Result<Self> {
        let conn = Connection::open_in_memory()?;
        let store = Self { conn, dim };
        store.init()?;
        Ok(store)
    }

    fn init(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS memories(
                id INTEGER PRIMARY KEY,
                kind TEXT NOT NULL,
                text TEXT NOT NULL,
                embedding BLOB NOT NULL,
                dim INTEGER NOT NULL,
                source TEXT NOT NULL DEFAULT '',
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL
            );
            CREATE INDEX IF NOT EXISTS idx_memories_kind ON memories(kind);
            CREATE TABLE IF NOT EXISTS meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);",
        )?;
        let stored_dim: Option<i64> = self
            .conn
            .query_row("SELECT value FROM meta WHERE key='dim'", [], |r| {
                r.get::<_, String>(0)
            })
            .ok()
            .and_then(|v| v.parse().ok());
        match stored_dim {
            Some(d) if d as usize != self.dim => anyhow::bail!(
                "Memory dim mismatch: store has {d}, embedder produces {}. \
                 Delete the .db file or align EMBEDDING_DIM to re-embed.",
                self.dim
            ),
            Some(_) => {}
            None => {
                self.conn.execute(
                    "INSERT INTO meta(key, value) VALUES('dim', ?1)",
                    params![self.dim.to_string()],
                )?;
            }
        }
        Ok(())
    }

    pub fn dim(&self) -> usize {
        self.dim
    }

    pub fn add(&self, text: &str, kind: &str, source: &str, embedding: &[f32]) -> anyhow::Result<i64> {
        if embedding.len() != self.dim {
            anyhow::bail!("Bad embedding dim: got {}, want {}.", embedding.len(), self.dim);
        }
        let now = now_ms();
        self.conn.execute(
            "INSERT INTO memories(kind, text, embedding, dim, source, created_at, updated_at)
             VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?6)",
            params![kind, text, encode_vec(embedding), self.dim as i64, source, now],
        )?;
        Ok(self.conn.last_insert_rowid())
    }

    pub fn recall(&self, embedding: &[f32], top_k: usize, min_sim: f32) -> Vec<ScoredMemory> {
        let mut stmt = match self.conn.prepare("SELECT id, kind, text, source, embedding FROM memories") {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        let rows = match stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Vec<u8>>(4)?,
            ))
        }) {
            Ok(r) => r,
            Err(_) => return vec![],
        };
        let mut scored = Vec::new();
        for row in rows.flatten() {
            let (id, kind, text, source, blob) = row;
            let vec = decode_vec(&blob);
            let sim = cosine(embedding, &vec);
            if sim >= min_sim {
                scored.push(ScoredMemory { id, kind, text, source, sim });
            }
        }
        scored.sort_by(|a, b| b.sim.partial_cmp(&a.sim).unwrap_or(std::cmp::Ordering::Equal));
        scored.truncate(top_k);
        scored
    }

    pub fn list_recent(&self, limit: usize) -> Vec<MemoryItem> {
        let mut stmt = match self
            .conn
            .prepare("SELECT id, kind, text, source, created_at FROM memories ORDER BY id DESC LIMIT ?1")
        {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map(params![limit as i64], |r| {
            Ok(MemoryItem {
                id: r.get(0)?,
                kind: r.get(1)?,
                text: r.get(2)?,
                source: r.get(3)?,
                created_at: r.get(4)?,
            })
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }

    pub fn delete(&self, id: i64) -> bool {
        self.conn
            .execute("DELETE FROM memories WHERE id=?1", params![id])
            .map(|n| n > 0)
            .unwrap_or(false)
    }

    /// Delete every memory of a kind. Returns rows removed.
    pub fn delete_by_kind(&self, kind: &str) -> usize {
        self.conn
            .execute("DELETE FROM memories WHERE kind=?1", params![kind])
            .unwrap_or(0)
    }

    pub fn meta_get(&self, key: &str) -> Option<String> {
        self.conn
            .query_row("SELECT value FROM meta WHERE key=?1", params![key], |r| {
                r.get::<_, String>(0)
            })
            .ok()
    }

    pub fn meta_set(&self, key: &str, value: &str) {
        let _ = self.conn.execute(
            "INSERT INTO meta(key, value) VALUES(?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value=excluded.value",
            params![key, value],
        );
    }

    pub fn count(&self) -> i64 {
        self.conn
            .query_row("SELECT COUNT(*) FROM memories", [], |r| r.get::<_, i64>(0))
            .unwrap_or(0)
    }

    pub fn kinds(&self) -> Vec<(String, i64)> {
        let mut stmt = match self.conn.prepare("SELECT kind, COUNT(*) FROM memories GROUP BY kind") {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))
            .map(|rows| rows.flatten().collect())
            .unwrap_or_default()
    }

    /// Projects live in the same store: kind='project', text=path, source=name.
    pub fn remember_project(&self, name: &str, project_path: &str, embedding: &[f32]) -> anyhow::Result<()> {
        let existing: Option<i64> = self
            .conn
            .query_row(
                "SELECT id FROM memories WHERE kind='project' AND lower(source)=lower(?1)",
                params![name],
                |r| r.get(0),
            )
            .ok();
        let now = now_ms();
        match existing {
            Some(id) => {
                self.conn.execute(
                    "UPDATE memories SET text=?1, embedding=?2, updated_at=?3 WHERE id=?4",
                    params![project_path, encode_vec(embedding), now, id],
                )?;
            }
            None => {
                self.conn.execute(
                    "INSERT INTO memories(kind, text, embedding, dim, source, created_at, updated_at)
                     VALUES('project', ?1, ?2, ?3, ?4, ?5, ?5)",
                    params![project_path, encode_vec(embedding), self.dim as i64, name, now],
                )?;
            }
        }
        Ok(())
    }

    pub fn resolve_project(&self, name: &str) -> Option<String> {
        self.conn
            .query_row(
                "SELECT text FROM memories WHERE kind='project' AND lower(source)=lower(?1)",
                params![name],
                |r| r.get::<_, String>(0),
            )
            .ok()
    }

    /// Snapshot for prompt injection: recent projects as "name -> path" lines.
    pub fn project_lines(&self) -> Vec<String> {
        let mut stmt = match self.conn.prepare(
            "SELECT source, text FROM memories WHERE kind='project' ORDER BY updated_at DESC LIMIT 50",
        ) {
            Ok(s) => s,
            Err(_) => return vec![],
        };
        stmt.query_map([], |r| {
            let name: String = r.get(0)?;
            let path: String = r.get(1)?;
            Ok(format!("{name} -> {path}"))
        })
        .map(|rows| rows.flatten().collect())
        .unwrap_or_default()
    }
}

// ---------- imports ----------
// Locking rule: these async fns NEVER hold the store mutex across `.await`.
// They embed first, then lock briefly for the synchronous insert.

type Item = (String, String, String); // (text, kind, source)

/// Embed items, dropping empties while keeping text/vector alignment.
async fn embed_items<E: Embedder>(
    embedder: &E,
    items: &[Item],
) -> anyhow::Result<Vec<(Item, Vec<f32>)>> {
    let live: Vec<&Item> = items.iter().filter(|(t, _, _)| !t.trim().is_empty()).collect();
    if live.is_empty() {
        return Ok(vec![]);
    }
    let texts: Vec<String> = live.iter().map(|(t, _, _)| (*t).clone()).collect();
    let vectors = embedder.embed(&texts).await?;
    if vectors.len() != live.len() {
        anyhow::bail!("EMBED_FAILED: got {} vectors for {} texts.", vectors.len(), live.len());
    }
    Ok(live.into_iter().cloned().zip(vectors).collect())
}

/// Insert pre-embedded items. Synchronous — caller locks only around this.
fn insert_embedded(store: &MemoryStore, pairs: &[(Item, Vec<f32>)]) -> anyhow::Result<Vec<i64>> {
    let mut ids = Vec::with_capacity(pairs.len());
    for ((text, kind, source), vec) in pairs {
        ids.push(store.add(text, kind, source, vec)?);
    }
    Ok(ids)
}

/// Embed + insert a batch of (text, kind, source) rows. Returns ids.
pub async fn store_texts<E: Embedder>(
    store: &SharedMemory,
    embedder: &E,
    items: &[Item],
) -> anyhow::Result<Vec<i64>> {
    if items.is_empty() {
        return Ok(vec![]);
    }
    let pairs = embed_items(embedder, items).await?;
    let guard = store.lock().map_err(|e| anyhow::anyhow!("Memory lock poisoned: {e}"))?;
    insert_embedded(&guard, &pairs)
}

/// Plain-text import (pasted memory): chunked, kind selectable.
pub async fn import_text<E: Embedder>(
    store: &SharedMemory,
    embedder: &E,
    text: &str,
    kind: &str,
    source: &str,
) -> anyhow::Result<usize> {
    let chunks = chunk_text(text, 1500);
    let items: Vec<Item> =
        chunks.into_iter().map(|c| (c, kind.to_string(), source.to_string())).collect();
    Ok(store_texts(store, embedder, &items).await?.len())
}

// Minimal ChatGPT conversations.json shape (lenient: everything optional).
#[derive(Debug, Deserialize, Default)]
struct GptExportAuthor {
    #[serde(default)]
    role: String,
}
#[derive(Debug, Deserialize, Default)]
struct GptExportContent {
    #[serde(default)]
    content_type: String,
    #[serde(default)]
    parts: Vec<serde_json::Value>,
}
#[derive(Debug, Deserialize, Default)]
struct GptExportMessage {
    #[serde(default)]
    author: GptExportAuthor,
    #[serde(default)]
    content: GptExportContent,
}
#[derive(Debug, Deserialize, Default)]
struct GptExportNode {
    #[serde(default)]
    message: Option<GptExportMessage>,
}
#[derive(Debug, Deserialize, Default)]
struct GptConversation {
    #[serde(default)]
    title: String,
    #[serde(default)]
    mapping: HashMap<String, GptExportNode>,
}

/// Import a ChatGPT `conversations.json` export. Returns memories stored.
pub async fn import_chatgpt_json<E: Embedder>(
    store: &SharedMemory,
    embedder: &E,
    json_text: &str,
) -> anyhow::Result<usize> {
    let convos: Vec<GptConversation> = serde_json::from_str(json_text)
        .map_err(|e| anyhow::anyhow!("CHATGPT_PARSE_FAILED: not a conversations.json export ({e})"))?;
    if convos.is_empty() {
        anyhow::bail!("CHATGPT_PARSE_FAILED: no conversations found.");
    }
    let mut items: Vec<Item> = Vec::new();
    for convo in &convos {
        let title = if convo.title.is_empty() { "untitled".to_string() } else { convo.title.clone() };
        let mut transcript = String::new();
        let mut nodes: Vec<(&String, &GptExportNode)> = convo.mapping.iter().collect();
        nodes.sort_by_key(|(k, _)| *k);
        for (_, node) in nodes {
            let Some(msg) = &node.message else { continue };
            if msg.content.content_type != "text" {
                continue;
            }
            let text: String = msg
                .content
                .parts
                .iter()
                .filter_map(|p| p.as_str())
                .collect::<Vec<_>>()
                .join("\n");
            if text.trim().is_empty() {
                continue;
            }
            let who = match msg.author.role.as_str() {
                "user" => "User",
                "assistant" => "Assistant",
                _ => continue,
            };
            transcript.push_str(&format!("{who}: {text}\n\n"));
        }
        if transcript.trim().is_empty() {
            continue;
        }
        for chunk in chunk_text(&transcript, 1500) {
            items.push((chunk, "imported".to_string(), format!("chatgpt:{title}")));
        }
    }
    if items.is_empty() {
        anyhow::bail!("CHATGPT_PARSE_FAILED: no text content found in export.");
    }
    Ok(store_texts(store, embedder, &items).await?.len())
}

/// One-time migration of the legacy JSON memory file into SQLite.
pub async fn migrate_legacy_json<E: Embedder>(
    store: &SharedMemory,
    embedder: &E,
    json_path: &Path,
) -> anyhow::Result<usize> {
    {
        let guard = store.lock().map_err(|e| anyhow::anyhow!("Memory lock poisoned: {e}"))?;
        if guard.count() > 0 || !json_path.exists() {
            return Ok(0);
        }
    }
    let content = std::fs::read_to_string(json_path)?;
    let raw: serde_json::Value = serde_json::from_str(&content)?;
    let mut items: Vec<Item> = Vec::new();
    for p in raw.get("projects").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let name = p.get("name").and_then(|v| v.as_str()).unwrap_or("");
        let path = p.get("path").and_then(|v| v.as_str()).unwrap_or("");
        if !name.is_empty() && !path.is_empty() {
            items.push((path.to_string(), "project".to_string(), name.to_string()));
        }
    }
    for t in raw.get("taskHistory").and_then(|v| v.as_array()).cloned().unwrap_or_default() {
        let req = t.get("request").and_then(|v| v.as_str()).unwrap_or("");
        let summary = t.get("summary").and_then(|v| v.as_str()).unwrap_or("");
        let intent = t.get("intent").and_then(|v| v.as_str()).unwrap_or("");
        if !req.is_empty() {
            items.push((format!("{req} => {summary}"), "task".to_string(), intent.to_string()));
        }
    }
    if items.is_empty() {
        return Ok(0);
    }
    let n = store_texts(store, embedder, &items).await?.len();
    let migrated = json_path.with_extension("json.migrated");
    let _ = std::fs::rename(json_path, migrated);
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct UnitEmbedder;
    impl Embedder for UnitEmbedder {
        async fn embed(&self, texts: &[String]) -> anyhow::Result<Vec<Vec<f32>>> {
            // Deterministic toy vectors: bucketed by keyword presence.
            Ok(texts
                .iter()
                .map(|t| {
                    let t = t.to_lowercase();
                    vec![
                        if t.contains("paris") { 1.0 } else { 0.0 },
                        if t.contains("likes") || t.contains("loves") { 1.0 } else { 0.0 },
                        if t.contains("project") { 1.0 } else { 0.0 },
                    ]
                })
                .collect())
        }
    }

    #[test]
    fn cosine_orders_by_similarity() {
        let store: SharedMemory = Arc::new(Mutex::new(MemoryStore::open_in_memory(3).unwrap()));
        let e = UnitEmbedder;
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            store_texts(
                &store,
                &e,
                &[
                    ("The capital of France is Paris.".into(), "fact".into(), "".into()),
                    ("Comrade project lives in /tmp/x.".into(), "project".into(), "x".into()),
                ],
            )
            .await
            .unwrap();
        });
        let q = vec![1.0, 0.0, 0.0];
        let hits = store.lock().unwrap().recall(&q, 5, 0.0);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].text.contains("Paris"));
    }

    #[test]
    fn projects_round_trip() {
        let store = MemoryStore::open_in_memory(3).unwrap();
        store.remember_project("Co-Founder", "/home/u/co-founder", &[0.0, 0.0, 1.0]).unwrap();
        assert_eq!(store.resolve_project("co-founder").as_deref(), Some("/home/u/co-founder"));
        assert_eq!(store.project_lines(), vec!["Co-Founder -> /home/u/co-founder".to_string()]);
    }

    #[test]
    fn chatgpt_import_parses_export() {
        let store: SharedMemory = Arc::new(Mutex::new(MemoryStore::open_in_memory(3).unwrap()));
        let e = UnitEmbedder;
        let json = serde_json::json!([
            {"title": "Trip", "mapping": {
                "a": {"message": {"author": {"role": "user"},
                    "content": {"content_type": "text", "parts": ["User loves Paris"]}}},
                "b": {"message": {"author": {"role": "assistant"},
                    "content": {"content_type": "text", "parts": ["Nice!"]}}},
                "c": {"message": null}
            }}
        ])
        .to_string();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let n = rt.block_on(import_chatgpt_json(&store, &e, &json)).unwrap();
        assert_eq!(n, 1);
        let guard = store.lock().unwrap();
        assert_eq!(guard.count(), 1);
        let items = guard.list_recent(5);
        assert!(items[0].source.contains("Trip"));
    }

    #[test]
    fn chunking_splits_long_text() {
        let text = "a".repeat(4000);
        let chunks = chunk_text(&text, 1500);
        assert!(chunks.len() >= 3);
        assert!(chunks.iter().all(|c| c.chars().count() <= 1500));
    }
}
