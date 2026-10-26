//! Same local schema, data, durability and bounded reads in both engines.
use anyhow::{Result, ensure};
use std::{
    collections::hash_map::DefaultHasher,
    hash::{Hash, Hasher},
    time::Instant,
};

const SESSIONS: i64 = 512;
const MESSAGES: i64 = 200;
const SCHEMA: &[&str] = &[
    "CREATE TABLE sessions(id INTEGER PRIMARY KEY, updated INTEGER NOT NULL, title TEXT NOT NULL)",
    "CREATE INDEX sessions_updated ON sessions(updated DESC,id)",
    "CREATE TABLE messages(session INTEGER NOT NULL,seq INTEGER NOT NULL,payload TEXT NOT NULL,PRIMARY KEY(session,seq))",
    "CREATE TABLE events(seq INTEGER PRIMARY KEY,payload TEXT NOT NULL)",
];
const LIST: &str = "SELECT id,title FROM sessions ORDER BY updated DESC,id LIMIT 50";
const PAGE: &str =
    "SELECT seq,payload FROM messages WHERE session=?1 AND seq<?2 ORDER BY seq DESC LIMIT 50";
fn ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
fn payload() -> String {
    serde_json::json!({"role":"user","text":"Синтетическое сообщение для сравнения хранилищ. ".repeat(16)}).to_string()
}
fn digest(rows: &[(i64, String)]) -> u64 {
    let mut h = DefaultHasher::new();
    rows.hash(&mut h);
    h.finish()
}
fn check(rows: &[(i64, String)], first: i64) {
    assert_eq!(rows.len(), 50);
    assert_eq!(rows.first().unwrap().0, first);
    assert_eq!(rows.last().unwrap().0, first - 49);
}

fn sqlite(mode: &str, path: &str) -> Result<serde_json::Value> {
    let started = Instant::now();
    let conn = rusqlite::Connection::open(path)?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "FULL")?;
    let open_ms = ms(started);
    match mode {
        "seed" => {
            for sql in SCHEMA {
                conn.execute_batch(sql)?;
            }
            conn.execute_batch("BEGIN IMMEDIATE")?;
            let text = payload();
            let mut session = conn.prepare("INSERT INTO sessions VALUES(?1,?1,?2)")?;
            let mut message = conn.prepare("INSERT INTO messages VALUES(?1,?2,?3)")?;
            for id in 0..SESSIONS {
                session.execute(rusqlite::params![id, format!("Session {id}")])?;
                for seq in 0..MESSAGES {
                    message.execute(rusqlite::params![id, seq, text])?;
                }
            }
            conn.execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE)")?;
            Ok(serde_json::json!({"seed_ms":ms(started)}))
        }
        "read" => {
            let started = Instant::now();
            let rows = conn
                .prepare_cached(LIST)?
                .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            check(&rows, SESSIONS - 1);
            let list_ms = ms(started);
            let list_digest = digest(&rows);
            let started = Instant::now();
            let mut statement = conn.prepare_cached(PAGE)?;
            let page = statement
                .query_map([SESSIONS - 1, MESSAGES], |r| Ok((r.get(0)?, r.get(1)?)))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            check(&page, MESSAGES - 1);
            let page_ms = ms(started);
            let page_digest = digest(&page);
            let started = Instant::now();
            let mut pages = 0;
            for _ in 0..100 {
                for cursor in (50..=MESSAGES).step_by(50) {
                    let rows = statement
                        .query_map([SESSIONS - 1, cursor], |r| Ok((r.get(0)?, r.get(1)?)))?
                        .collect::<rusqlite::Result<Vec<_>>>()?;
                    check(&rows, cursor - 1);
                    std::hint::black_box(digest(&rows));
                    pages += 1;
                }
            }
            Ok(
                serde_json::json!({"open_ms":open_ms,"first_list_ms":list_ms,"first_page_ms":page_ms,"warm_page_mean_ms":ms(started)/pages as f64,"list_digest":list_digest,"page_digest":page_digest}),
            )
        }
        "write" => {
            let started = Instant::now();
            let text = payload();
            for _ in 0..100 {
                conn.execute("INSERT INTO events(payload) VALUES(?1)", [&text])?;
            }
            let count: i64 = conn.query_row("SELECT count(*) FROM events", [], |r| r.get(0))?;
            ensure!(count >= 100);
            Ok(serde_json::json!({"durable_100_events_ms":ms(started)}))
        }
        _ => anyhow::bail!("unknown mode"),
    }
}

async fn turso(mode: &str, path: &str) -> Result<serde_json::Value> {
    let started = Instant::now();
    let db = turso::Builder::new_local(path).build().await?;
    let conn = db.connect()?;
    for sql in ["PRAGMA journal_mode=WAL", "PRAGMA synchronous=FULL"] {
        let mut rows = conn.query(sql, ()).await?;
        while rows.next().await?.is_some() {}
    }
    let open_ms = ms(started);
    match mode {
        "seed" => {
            for sql in SCHEMA {
                conn.execute(sql, ()).await?;
            }
            conn.execute("BEGIN IMMEDIATE", ()).await?;
            let text = payload();
            let mut session = conn
                .prepare("INSERT INTO sessions VALUES(?1,?1,?2)")
                .await?;
            let mut message = conn
                .prepare("INSERT INTO messages VALUES(?1,?2,?3)")
                .await?;
            for id in 0..SESSIONS {
                session.execute((id, format!("Session {id}"))).await?;
                for seq in 0..MESSAGES {
                    message.execute((id, seq, text.as_str())).await?;
                }
            }
            conn.execute("COMMIT", ()).await?;
            drop(message);
            drop(session);
            let mut rows = conn.query("PRAGMA wal_checkpoint(TRUNCATE)", ()).await?;
            while rows.next().await?.is_some() {}
            Ok(serde_json::json!({"seed_ms":ms(started)}))
        }
        "read" => {
            let started = Instant::now();
            let rows = collect(conn.query(LIST, ()).await?).await?;
            check(&rows, SESSIONS - 1);
            let list_ms = ms(started);
            let list_digest = digest(&rows);
            let started = Instant::now();
            let mut statement = conn.prepare(PAGE).await?;
            let page = collect(statement.query((SESSIONS - 1, MESSAGES)).await?).await?;
            check(&page, MESSAGES - 1);
            let page_ms = ms(started);
            let page_digest = digest(&page);
            let started = Instant::now();
            let mut pages = 0;
            for _ in 0..100 {
                for cursor in (50..=MESSAGES).step_by(50) {
                    let rows = collect(statement.query((SESSIONS - 1, cursor)).await?).await?;
                    check(&rows, cursor - 1);
                    std::hint::black_box(digest(&rows));
                    pages += 1;
                }
            }
            Ok(
                serde_json::json!({"open_ms":open_ms,"first_list_ms":list_ms,"first_page_ms":page_ms,"warm_page_mean_ms":ms(started)/pages as f64,"list_digest":list_digest,"page_digest":page_digest}),
            )
        }
        "write" => {
            let started = Instant::now();
            let text = payload();
            for _ in 0..100 {
                conn.execute("INSERT INTO events(payload) VALUES(?1)", [text.as_str()])
                    .await?;
            }
            let rows = collect(conn.query("SELECT count(*),'' FROM events", ()).await?).await?;
            ensure!(rows[0].0 >= 100);
            Ok(serde_json::json!({"durable_100_events_ms":ms(started)}))
        }
        _ => anyhow::bail!("unknown mode"),
    }
}
async fn collect(mut rows: turso::Rows) -> Result<Vec<(i64, String)>> {
    let mut out = Vec::new();
    while let Some(row) = rows.next().await? {
        out.push((row.get(0)?, row.get(1)?));
    }
    Ok(out)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args = std::env::args().collect::<Vec<_>>();
    ensure!(
        args.len() == 4,
        "usage: session-db-bench sqlite|turso seed|read|write DATABASE"
    );
    let started = Instant::now();
    let mut result = match args[1].as_str() {
        "sqlite" => sqlite(&args[2], &args[3])?,
        "turso" => turso(&args[2], &args[3]).await?,
        _ => anyhow::bail!("unknown engine"),
    };
    result["engine"] = args[1].clone().into();
    result["mode"] = args[2].clone().into();
    result["total_ms"] = ms(started).into();
    println!("{result}");
    Ok(())
}
