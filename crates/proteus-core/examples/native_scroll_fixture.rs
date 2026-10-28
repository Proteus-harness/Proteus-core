//! Длинный синтетический чат для ручной проверки нативной прокрутки.
//! Запуск: `native_scroll_fixture CONFIG_ROOT WORKSPACE ROWS [--folded-tail]`.
//! Создаёт новую сессию; модели, инструменты и внешние API не вызывает.
//! `--folded-tail` оставляет в конце 50 пар вызов/результат без финального текста:
//! нативный клиент сворачивает этот хвост в одну строку цепочки инструментов.

use anyhow::{Result, ensure};
use proteus_contracts::{
    domain::{ToolCall, ToolResult, new_call_id, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, ContentPart, MessageRole},
};
use proteus_core::core::SessionStore;
use std::{fmt::Write, path::PathBuf};

const FOLDED_TOOL_PAIRS: usize = 50;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        args.len() == 4 || args.len() == 5,
        "native_scroll_fixture CONFIG_ROOT WORKSPACE ROWS [--folded-tail]"
    );
    let folded_tail = match args.get(4).map(String::as_str) {
        None => false,
        Some("--folded-tail") => true,
        Some(other) => anyhow::bail!("unknown option: {other}"),
    };
    let root = PathBuf::from(&args[1]);
    let cwd = PathBuf::from(&args[2]);
    let rows: usize = args[3].parse()?;
    ensure!(rows >= 2, "ROWS must be at least 2");
    ensure!(
        !folded_tail || rows > FOLDED_TOOL_PAIRS * 2,
        "--folded-tail requires ROWS > {}",
        FOLDED_TOOL_PAIRS * 2
    );
    std::fs::create_dir_all(&cwd)?;
    let store = SessionStore::new(&root, &cwd, new_session_id())?;
    let title = format!(
        "{} — {} сообщений",
        if folded_tail {
            "Тест короткого хвоста"
        } else {
            "Тест прокрутки"
        },
        grouped_number(rows)
    );
    let text_rows = if folded_tail {
        rows - FOLDED_TOOL_PAIRS * 2
    } else {
        rows
    };
    let mut messages: Vec<_> = (0..text_rows)
        .map(|index| {
            let role = if index % 2 == 0 && index != rows - 1 {
                MessageRole::User
            } else {
                MessageRole::Assistant
            };
            let text = if index == 0 {
                title.clone()
            } else if index == rows - 1 {
                code_message(index, 1_000)
            } else {
                match index % 3 {
                    0 => format!("Сообщение {index}: короткая строка для проверки прокрутки."),
                    1 => format!(
                        "Сообщение {index}: несколько абзацев.\n\n{}\n\n{}",
                        "Синтетический текст разной высоты проверяет движение между сообщениями. "
                            .repeat(6),
                        "Прокрутка должна работать колесом, ползунком и при выделении текста. "
                            .repeat(4)
                    ),
                    _ => code_message(index, 32),
                }
            };
            CanonicalMessage::text(role, text)
        })
        .collect();
    if folded_tail {
        append_folded_tail(&mut messages);
    }
    // Одна большая запись на 50 000 сообщений превышает лимит journal payload.
    // Сохраняем одну беседу небольшими порциями через тот же typed writer.
    let thread_id = new_thread_id();
    for chunk in messages.chunks(1_000) {
        store.append_history(thread_id, None, chunk).await?;
    }

    // Проверяем именно сохранённую canonical history, а не счётчик генератора.
    let saved = SessionStore::open(store.session_dir().to_path_buf())?.load_messages()?;
    ensure!(saved == messages, "saved history differs from fixture");
    println!(
        "{}",
        serde_json::json!({"session_dir": store.session_dir(), "rows": saved.len()})
    );
    Ok(())
}

fn append_folded_tail(messages: &mut Vec<CanonicalMessage>) {
    for index in 0..FOLDED_TOOL_PAIRS {
        let call = ToolCall::new(
            new_call_id(),
            "read_file",
            serde_json::json!({"path": format!("synthetic-scroll-{index}.txt")}),
        );
        messages.push(CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall { call: call.clone() }],
        ));
        messages.push(
            CanonicalMessage::new(
                MessageRole::Tool,
                vec![ContentPart::ToolResult {
                    result: ToolResult::ok(call.id.clone(), "Синтетический результат чтения."),
                }],
            )
            .with_tool_call_id(call.id),
        );
    }
}

fn grouped_number(number: usize) -> String {
    let digits = number.to_string();
    digits
        .as_bytes()
        .rchunks(3)
        .rev()
        .map(|chunk| std::str::from_utf8(chunk).expect("decimal digits"))
        .collect::<Vec<_>>()
        .join(" ")
}

fn code_message(index: usize, lines: usize) -> String {
    let mut text = format!("Сообщение {index}: блок кода, {lines} строк.\n\n```rust\n");
    for line in 0..lines {
        writeln!(
            text,
            "let synthetic_{line:04} = \"сообщение {index}, строка {line}: проверка длинного блока\";"
        )
        .expect("writing to String");
    }
    text.push_str("```\n\nКонец синтетического сообщения.");
    text
}
