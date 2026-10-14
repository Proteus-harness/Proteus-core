//! Form of a profile MCP server inside its row on the MCP page: the main
//! fields, the rarely needed ones folded, and the removal of the server.
//! Its text editors live in `mcp_editors`.

use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::input::{Input, Textarea};
use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::domain::ConfiguredMcpServerConfig;

use super::SettingsView;
use super::editors::TextInput;
use super::mcp::mcp_advanced_key;
use super::widgets::{
    Fit, MenuOption, SHORT_INPUT_WIDTH, changed_dot, described, disclosure, field_row, hashed_id,
    select_menu,
};
use crate::settings::mcp::{
    McpProblem, McpText, SAFETY_TITLES, mcp_problems, mcp_text, safety_title,
};
use crate::ui::kit;
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

const MAIN: [McpText; 5] = [
    McpText::Name,
    McpText::Command,
    McpText::Args,
    McpText::Cwd,
    McpText::Env,
];
const ADVANCED: [McpText; 4] = [
    McpText::EnvAllowlist,
    McpText::TimeoutMs,
    McpText::MaxResponseBytes,
    McpText::ProtocolVersion,
];

/// Title, tooltip and profile key of a text field.
fn describe_field(field: McpText) -> (&'static str, &'static str, &'static str) {
    match field {
        McpText::Name => (
            "Имя",
            "Имя сервера в профиле. Инструменты сервера называются имя__инструмент.",
            "name",
        ),
        McpText::Command => (
            "Команда",
            "Программа, которая запускает сервер; без пути ищется в PATH.",
            "command",
        ),
        McpText::Args => (
            "Аргументы",
            "Аргументы команды, по одному на строку.",
            "args",
        ),
        McpText::Cwd => (
            "Рабочая папка",
            "Папка запуска сервера. Относительный путь считается от рабочей папки агента, пустое поле — она сама.",
            "cwd",
        ),
        McpText::Env => (
            "Переменные окружения",
            "Свои значения переменных для сервера, по строке KEY=VALUE.",
            "env",
        ),
        McpText::EnvAllowlist => (
            "Наследуемые переменные",
            "Имена переменных окружения агента, которые сервер получит вдобавок к базовым вроде PATH, по одному на строку.",
            "env_allowlist",
        ),
        McpText::TimeoutMs => (
            "Тайм-аут, мс",
            "Сколько ждать ответа сервера. Пустое поле — значение runtime по умолчанию.",
            "timeout_ms",
        ),
        McpText::MaxResponseBytes => (
            "Предел ответа, байт",
            "Наибольший размер ответа сервера. Пустое поле — значение runtime по умолчанию.",
            "max_response_bytes",
        ),
        McpText::ProtocolVersion => (
            "Версия протокола",
            "Версия MCP, которую клиент предлагает серверу при подключении.",
            "protocol_version",
        ),
    }
}

fn is_short(field: McpText) -> bool {
    matches!(field, McpText::TimeoutMs | McpText::MaxResponseBytes)
}

/// Title of a form row with its tooltip and the mark of an unsaved edit.
fn row_title(
    id: ElementId,
    title: &'static str,
    about: &'static str,
    key: &'static str,
    changed: bool,
    cx: &App,
) -> Div {
    h_flex()
        .min_w_0()
        .gap_1()
        .items_center()
        .child(described(
            id,
            title.into(),
            Some(about.into()),
            Some(format!("tools.mcp_servers.{key}").into()),
            cx,
        ))
        .when(changed, |title| title.child(changed_dot(cx)))
}

/// Rows one under another, divided by hairlines.
fn stacked(rows: Vec<AnyElement>, cx: &App) -> Div {
    let border = cx.theme().border;
    v_flex().children(rows.into_iter().enumerate().map(|(ix, row)| {
        div()
            .when(ix > 0, |cell| cell.border_t_1().border_color(border))
            .child(row)
    }))
}

impl SettingsView {
    /// Fields of the profile server at `ix`, the rarely needed ones folded,
    /// and its removal.
    pub(super) fn mcp_form(
        &self,
        ix: usize,
        server: &ConfiguredMcpServerConfig,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let saved = self.draft.mcp_baseline(&self.baseline, ix);
        let problems = mcp_problems(&self.draft, self.addons());
        let text_row = |field: McpText| self.mcp_text_row(ix, field, server, saved, &problems, cx);
        let mut rows: Vec<AnyElement> = MAIN.into_iter().map(&text_row).collect();
        rows.push(self.mcp_safety_row(ix, server, saved, cx));

        let key = mcp_advanced_key(&server.name);
        let changed = saved.is_some_and(|saved| {
            saved.supports_parallel_tool_calls != server.supports_parallel_tool_calls
                || ADVANCED
                    .into_iter()
                    .any(|field| mcp_text(saved, field) != mcp_text(server, field))
        });
        let view = cx.weak_entity();
        let toggle_key = key.clone();
        rows.push(
            disclosure(
                hashed_id("mcp-advanced", &ix),
                self.is_open(&key),
                format!("Дополнительно · {}", ADVANCED.len() + 1),
                changed,
                move |cx| {
                    let _ = view.update(cx, |this, cx| this.toggle_open(&toggle_key, cx));
                },
                cx,
            )
            .into_any_element(),
        );
        rows.extend(self.reveal(&key, cx, || {
            let advanced = std::iter::once(self.mcp_parallel_row(ix, server, saved, cx))
                .chain(ADVANCED.into_iter().map(&text_row))
                .collect();
            stacked(advanced, cx).into_any_element()
        }));
        rows.push(
            h_flex()
                .px_4()
                .py_2()
                .justify_end()
                .child(
                    kit::button(hashed_id("mcp-remove", &ix))
                        .icon(Lucide::Trash)
                        .label("Удалить сервер")
                        .ghost()
                        .xsmall()
                        .hint("Убрать сервер из профиля при сохранении", cx)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.remove_mcp_server(ix, cx)),
                        ),
                )
                .into_any_element(),
        );
        div()
            .ml(prefs::scaled(24.))
            .mb_2()
            .border_l_2()
            .border_color(theme.border)
            .child(stacked(rows, cx))
            .into_any_element()
    }

    fn mcp_text_row(
        &self,
        ix: usize,
        field: McpText,
        server: &ConfiguredMcpServerConfig,
        saved: Option<&ConfiguredMcpServerConfig>,
        problems: &[McpProblem],
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let form = self.editors.mcp.get(&ix);
        let short = is_short(field);
        let fit = if field.is_multiline() {
            Fit::Below
        } else if short {
            Fit::Fixed
        } else {
            Fit::Shrink
        };
        let control = match form.and_then(|form| form.inputs.get(&field)) {
            Some(TextInput::Line(state)) => Input::new(state)
                .small()
                .map(|input| {
                    if short {
                        input.w(prefs::scaled(SHORT_INPUT_WIDTH))
                    } else {
                        input.w_full()
                    }
                })
                .into_any_element(),
            Some(TextInput::Area(state)) => {
                Textarea::new(state).small().w_full().into_any_element()
            }
            None => div().into_any_element(),
        };
        let error = form
            .and_then(|form| form.errors.get(&field).cloned())
            .or_else(|| {
                problems
                    .iter()
                    .find(|problem| problem.index == ix && problem.field == field)
                    .map(|problem| problem.message.into())
            });
        let control = v_flex()
            .gap_1()
            .map(|column| {
                if short {
                    column.items_end()
                } else {
                    column.w_full()
                }
            })
            .child(control)
            .children(error.map(|error| div().text_xs().text_color(theme.danger).child(error)));
        let (title, about, key) = describe_field(field);
        let changed = saved.is_some_and(|saved| mcp_text(saved, field) != mcp_text(server, field));
        let title = row_title(
            hashed_id("mcp-field", &(ix, key)),
            title,
            about,
            key,
            changed,
            cx,
        );
        field_row(title, control, fit).into_any_element()
    }

    fn mcp_safety_row(
        &self,
        ix: usize,
        server: &ConfiguredMcpServerConfig,
        saved: Option<&ConfiguredMcpServerConfig>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let options = SAFETY_TITLES
            .iter()
            .map(|(safety, title)| MenuOption {
                label: (*title).into(),
                value: safety.clone(),
                checked: *safety == server.safety,
            })
            .collect();
        let view = cx.weak_entity();
        let menu = select_menu(
            hashed_id("mcp-safety", &ix),
            safety_title(&server.safety),
            options,
            move |safety, cx| {
                let _ = view.update(cx, |this, cx| {
                    this.update_draft(cx, |draft| {
                        draft.edit_mcp(ix, |server| server.safety = safety)
                    })
                });
            },
        );
        let title = row_title(
            hashed_id("mcp-field", &(ix, "safety")),
            "Безопасность",
            "Класс действий инструментов сервера для политики подтверждений. Для MCP «Только чтение» и «Пишет файлы» считаются как «Запускает команды».",
            "safety",
            saved.is_some_and(|saved| saved.safety != server.safety),
            cx,
        );
        field_row(title, menu, Fit::Fixed).into_any_element()
    }

    fn mcp_parallel_row(
        &self,
        ix: usize,
        server: &ConfiguredMcpServerConfig,
        saved: Option<&ConfiguredMcpServerConfig>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let parallel = server.supports_parallel_tool_calls;
        let switch = kit::switch(hashed_id("mcp-parallel", &ix))
            .checked(parallel)
            .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                let checked = *checked;
                this.update_draft(cx, |draft| {
                    draft.edit_mcp(ix, |server| server.supports_parallel_tool_calls = checked)
                });
            }));
        let title = row_title(
            hashed_id("mcp-field", &(ix, "parallel")),
            "Параллельные вызовы",
            "Модель может вызывать инструменты сервера одновременно. Инструменты с пометкой readOnlyHint параллельны и без этого.",
            "supports_parallel_tool_calls",
            saved.is_some_and(|saved| saved.supports_parallel_tool_calls != parallel),
            cx,
        );
        field_row(title, switch, Fit::Fixed).into_any_element()
    }
}
