use anyhow::Result;
use serde_json::{Value, json};
use std::{
    collections::VecDeque,
    fs::File,
    io::{BufRead, BufReader, BufWriter, Write},
    path::Path,
};

pub const PAGE: i64 = 50;
pub const SCHEMA: &str = "CREATE TABLE messages(seq INTEGER PRIMARY KEY,payload TEXT NOT NULL)";
pub const ALL: &str = "SELECT seq,payload FROM messages ORDER BY seq DESC";
pub const PAGED: &str = "SELECT seq,payload FROM messages WHERE seq<?1 ORDER BY seq DESC LIMIT 50";

pub fn seed(dir: &Path, rows: i64) -> Result<Value> {
    let path = dir.join("history.jsonl");
    let mut out = BufWriter::new(File::create_new(&path)?);
    for seq in 0..rows {
        // AppTranscriptMessage-shaped synthetic content; no private sessions or inline blobs.
        let tool = seq % 8 == 7;
        let value = json!({"seq": seq, "message": {
            "message_id": format!("00000000-0000-0000-0000-{seq:012x}"),
            "phase": null, "role": if seq % 2 == 0 {"user"} else {"assistant"},
            "text": if tool {String::new()} else {"Синтетический текст сообщения; проверка Unicode. ".repeat(32)},
            "images": [], "streaming": false, "subagent": null,
            "tool": if tool {json!({"call_id": format!("call-{seq}"), "name": "read_file",
                "args": {"path": "synthetic.rs"}, "effective_args": null, "status": "done",
                "result": "fn synthetic() { /* fixture */ }\n".repeat(512), "metadata": {}})} else {Value::Null}
        }});
        serde_json::to_writer(&mut out, &value)?;
        out.write_all(b"\n")?;
    }
    out.flush()?;
    out.get_ref().sync_all()?;
    Ok(json!({"file_bytes": path.metadata()?.len()}))
}

pub fn lines(dir: &Path) -> Result<impl Iterator<Item = std::io::Result<String>>> {
    Ok(BufReader::new(File::open(dir.join("history.jsonl"))?).lines())
}

pub fn read(dir: &Path, rows: i64, mode: &str) -> Result<Value> {
    let mut read = crate::measure::Read::new()?;
    let input = lines(dir)?;
    read.opened()?;
    let mut retained = VecDeque::new();
    for (seq, payload) in input.enumerate() {
        if mode == "paged" && retained.len() == PAGE as usize {
            retained.pop_front();
        }
        retained.push_back(read.row(seq as i64, &payload?)?);
    }
    read.ready(retained.make_contiguous(), rows)?;
    let mut report = read.finish(rows)?;
    std::hint::black_box(&retained);
    drop(retained);
    report["memory"]["released"] = crate::measure::memory()?;
    Ok(report)
}
