//! Controlled storage/access-pattern experiment, not a Proteus runtime replacement.
#[path = "../memory/fixture.rs"]
mod fixture;
#[path = "../memory/measure.rs"]
mod measure;
#[path = "../memory/sql.rs"]
mod sql;

use anyhow::{Result, ensure};
use std::path::Path;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 5,
        "session-memory jsonl|sqlite|turso seed|eager|paged DIRECTORY ROWS"
    );
    let rows: i64 = args[4].parse()?;
    ensure!(rows >= fixture::PAGE, "at least one full page is required");
    let dir = Path::new(&args[3]);
    let mut report = match (args[1].as_str(), args[2].as_str()) {
        ("jsonl", "seed") => fixture::seed(dir, rows)?,
        ("jsonl", mode @ ("eager" | "paged")) => fixture::read(dir, rows, mode)?,
        ("sqlite", mode) => sql::sqlite(dir, rows, mode)?,
        ("turso", mode) => sql::turso(dir, rows, mode).await?,
        _ => anyhow::bail!("unknown engine or mode"),
    };
    report["engine"] = args[1].clone().into();
    report["mode"] = args[2].clone().into();
    report["rows"] = rows.into();
    println!("{report}");
    Ok(())
}
