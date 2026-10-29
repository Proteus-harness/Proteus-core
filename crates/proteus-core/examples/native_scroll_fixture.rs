//! Длинный синтетический чат для ручной проверки нативной прокрутки.
//! Запуск: `native_scroll_fixture CONFIG_ROOT WORKSPACE ROWS [--folded-tail|--tool-details|--content FILE]`.
//! Создаёт новую сессию; модели, инструменты и внешние API не вызывает.
//! `--folded-tail` оставляет в конце 50 пар вызов/результат без финального текста:
//! нативный клиент сворачивает этот хвост в одну строку цепочки инструментов.
//! `--tool-details` добавляет разные параметры одноимённых tools и длинный diff.

use anyhow::{Result, ensure};
use proteus_contracts::{
    domain::{ToolCall, ToolResult, new_call_id, new_session_id, new_thread_id},
    model_standard::{CanonicalMessage, ContentPart, MessageRole},
};
use proteus_core::core::SessionStore;
use std::{fmt::Write, path::PathBuf};

const FOLDED_TOOL_PAIRS: usize = 50;
const TOOL_DETAILS_MIN_ROWS: usize = 12;

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    ensure!(
        (4..=6).contains(&args.len()),
        "native_scroll_fixture CONFIG_ROOT WORKSPACE ROWS [--folded-tail|--tool-details|--content FILE]"
    );
    let (folded_tail, tool_details, content) = match args.get(4).map(String::as_str) {
        None if args.len() == 4 => (false, false, None),
        Some("--folded-tail") if args.len() == 5 => (true, false, None),
        Some("--tool-details") if args.len() == 5 => (false, true, None),
        Some("--content") if args.len() == 6 => {
            (false, false, Some(std::fs::read_to_string(&args[5])?))
        }
        other => anyhow::bail!("invalid fixture option: {other:?}"),
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
    ensure!(
        !tool_details || rows >= TOOL_DETAILS_MIN_ROWS,
        "--tool-details requires ROWS >= {TOOL_DETAILS_MIN_ROWS}"
    );
    std::fs::create_dir_all(&cwd)?;
    let store = SessionStore::new(&root, &cwd, new_session_id())?;
    let title = format!(
        "{} — {} сообщений",
        if tool_details {
            "Тест параметров инструментов и diff"
        } else if folded_tail {
            "Тест короткого хвоста"
        } else {
            "Тест прокрутки"
        },
        grouped_number(rows)
    );
    let text_rows = if tool_details {
        rows - (TOOL_DETAILS_MIN_ROWS - 1)
    } else if folded_tail {
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
            } else if index == rows - 1 && content.is_some() {
                content.as_ref().unwrap().clone()
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
    if tool_details {
        append_tool_details(&mut messages);
    }
    ensure!(
        messages.len() == rows,
        "generated message count differs from ROWS"
    );
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

fn append_tool_details(messages: &mut Vec<CanonicalMessage>) {
    let mut long_command = String::from("set -eu\n");
    for index in 0..80 {
        writeln!(
            long_command,
            "printf '%s\\n' 'Синтетическая строка {index:02}: длинный параметр, который только показывается в UI.'"
        )
        .expect("writing to String");
    }
    let cases = [
        (
            "exec_command",
            serde_json::json!({
                "cmd": "printf 'Привет, параметры\\n'\nprintf '%s\\n' \"$PWD\"",
                "cwd": "/synthetic/workspace/проект α",
                "timeout": 30_000,
            }),
        ),
        (
            "exec_command",
            serde_json::json!({
                "cmd": long_command,
                "cwd": "/synthetic/workspace/другой проект β",
                "timeout": 120_000,
            }),
        ),
        (
            "synthetic_inspect",
            serde_json::json!({
                "path": "/synthetic/данные/世界.json",
                "options": {
                    "labels": ["Кириллица", "日本語", "emoji 🧪"],
                    "limits": {"rows": 24, "ratio": 0.5},
                    "enabled": true,
                    "note": "Первая строка вложенного параметра.\nВторая строка: кавычки \" и путь C:\\synthetic.\n".repeat(40),
                },
                "nullable": null,
            }),
        ),
        (
            "apply_patch",
            serde_json::json!({"patch": tool_details_patch()}),
        ),
    ];
    for text in [
        "Параметры одноимённых tools отличаются; откройте подсказку у exec_command.",
        "Вложенные параметры содержат Unicode и длинный текст; подсказка ограничена.",
        "В apply_patch два файла: сравните Unified и Split и сворачивание длинного diff.",
    ] {
        messages.push(CanonicalMessage::text(MessageRole::Assistant, text));
    }
    for (name, args) in cases {
        let call = ToolCall::new(new_call_id(), name, args);
        messages.push(CanonicalMessage::new(
            MessageRole::Assistant,
            vec![ContentPart::ToolCall { call: call.clone() }],
        ));
        messages.push(
            CanonicalMessage::new(
                MessageRole::Tool,
                vec![ContentPart::ToolResult {
                    result: ToolResult::ok(
                        call.id.clone(),
                        "Синтетический результат; tool не запускался.",
                    ),
                }],
            )
            .with_tool_call_id(call.id),
        );
    }
}

fn tool_details_patch() -> String {
    let mut patch = String::from(
        "*** Begin Patch\n*** Update File: src/пример.rs\n@@\n // Общий контекст до изменений.\n-fn status() -> &'static str { \"старое\" }\n+fn status() -> &'static str { \"новое\" }\n",
    );
    for index in 0..80 {
        writeln!(
            patch,
            "-const ITEM_{index:02}: &str = \"старое значение {index}\";"
        )
        .expect("writing to String");
        writeln!(
            patch,
            "+const ITEM_{index:02}: &str = \"новое значение {index}\";"
        )
        .expect("writing to String");
    }
    patch.push_str(
        " fn stable_context() {}\n*** Update File: settings/пример.toml\n@@\n # Второй файл для сравнения Split и Unified.\n-theme = \"старое\"\n+theme = \"новое\"\n-enabled = false\n+enabled = true\n retries = 3\n*** End Patch",
    );
    patch
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
