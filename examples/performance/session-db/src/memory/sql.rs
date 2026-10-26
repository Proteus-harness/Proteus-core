use crate::{
    fixture,
    measure::{self, Read},
};
use anyhow::{Result, ensure};
use serde_json::{Value, json};
use std::path::Path;

const PRAGMAS: [&str; 3] = [
    "PRAGMA journal_mode=WAL",
    "PRAGMA synchronous=FULL",
    "PRAGMA cache_size=-2048",
];

pub fn sqlite(dir: &Path, count: i64, mode: &str) -> Result<Value> {
    let mut read = Read::new()?;
    let conn = rusqlite::Connection::open(dir.join("sqlite.db"))?;
    for pragma in PRAGMAS {
        conn.execute_batch(pragma)?;
    }
    let cache: i64 = conn.query_row("PRAGMA cache_size", [], |r| r.get(0))?;
    ensure!(cache == -2048);
    read.opened()?;
    if mode == "seed" {
        conn.execute_batch(fixture::SCHEMA)?;
        conn.execute_batch("BEGIN IMMEDIATE")?;
        let mut insert = conn.prepare("INSERT INTO messages VALUES(?1,?2)")?;
        for (seq, payload) in fixture::lines(dir)?.enumerate() {
            insert.execute((seq as i64, payload?))?;
        }
        drop(insert);
        conn.execute_batch("COMMIT; PRAGMA wal_checkpoint(TRUNCATE)")?;
        return Ok(json!({"file_bytes": dir.join("sqlite.db").metadata()?.len()}));
    }
    ensure!(mode == "eager" || mode == "paged", "unknown mode");
    let total: i64 = conn.query_row("SELECT count(*) FROM messages", [], |r| r.get(0))?;
    ensure!(total == count);
    let mut retained = Vec::new();
    let mut statement = conn.prepare(if mode == "eager" {
        fixture::ALL
    } else {
        fixture::PAGED
    })?;
    let mut cursor = count;
    loop {
        retained.clear();
        let mut rows = if mode == "eager" {
            statement.query([])?
        } else {
            statement.query([cursor])?
        };
        while let Some(row) = rows.next()? {
            let seq: i64 = row.get(0)?;
            ensure!(seq == cursor - 1, "noncontiguous page");
            cursor = seq;
            retained.push(read.row(seq, &row.get::<_, String>(1)?)?);
        }
        if cursor
            == if mode == "eager" {
                0
            } else {
                count - fixture::PAGE
            }
        {
            read.ready(&retained, count)?;
        }
        if cursor == 0 {
            break;
        }
        ensure!(!retained.is_empty(), "missing history page");
    }
    let mut report = read.finish(count)?;
    std::hint::black_box(&retained);
    drop(retained);
    report["memory"]["released"] = measure::memory()?;
    report["cache_kib"] = 2048.into();
    Ok(report)
}

pub async fn turso(dir: &Path, count: i64, mode: &str) -> Result<Value> {
    let mut read = Read::new()?;
    let db = turso::Builder::new_local(dir.join("turso.db").to_str().unwrap())
        .build()
        .await?;
    let conn = db.connect()?;
    for pragma in PRAGMAS {
        let mut rows = conn.query(pragma, ()).await?;
        while rows.next().await?.is_some() {}
    }
    let mut cache = conn.query("PRAGMA cache_size", ()).await?;
    ensure!(cache.next().await?.unwrap().get::<i64>(0)? == -2048);
    drop(cache);
    read.opened()?;
    if mode == "seed" {
        conn.execute(fixture::SCHEMA, ()).await?;
        conn.execute("BEGIN IMMEDIATE", ()).await?;
        let mut insert = conn.prepare("INSERT INTO messages VALUES(?1,?2)").await?;
        for (seq, payload) in fixture::lines(dir)?.enumerate() {
            insert.execute((seq as i64, payload?)).await?;
        }
        drop(insert);
        conn.execute("COMMIT", ()).await?;
        let mut rows = conn.query("PRAGMA wal_checkpoint(TRUNCATE)", ()).await?;
        while rows.next().await?.is_some() {}
        return Ok(json!({"file_bytes": dir.join("turso.db").metadata()?.len()}));
    }
    ensure!(mode == "eager" || mode == "paged", "unknown mode");
    let mut total = conn.query("SELECT count(*) FROM messages", ()).await?;
    ensure!(total.next().await?.unwrap().get::<i64>(0)? == count);
    drop(total);
    let mut retained = Vec::new();
    let mut statement = conn
        .prepare(if mode == "eager" {
            fixture::ALL
        } else {
            fixture::PAGED
        })
        .await?;
    let mut cursor = count;
    loop {
        retained.clear();
        let mut rows = if mode == "eager" {
            statement.query(()).await?
        } else {
            statement.query([cursor]).await?
        };
        while let Some(row) = rows.next().await? {
            let seq: i64 = row.get(0)?;
            ensure!(seq == cursor - 1, "noncontiguous page");
            cursor = seq;
            retained.push(read.row(seq, &row.get::<String>(1)?)?);
        }
        if cursor
            == if mode == "eager" {
                0
            } else {
                count - fixture::PAGE
            }
        {
            read.ready(&retained, count)?;
        }
        if cursor == 0 {
            break;
        }
        ensure!(!retained.is_empty(), "missing history page");
    }
    let mut report = read.finish(count)?;
    std::hint::black_box(&retained);
    drop(retained);
    report["memory"]["released"] = measure::memory()?;
    report["cache_kib"] = 2048.into();
    Ok(report)
}
