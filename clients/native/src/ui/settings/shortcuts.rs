//! «Сочетания клавиш»: window command shortcuts, recorded from the keyboard.

use crate::ui::kit;
use gpui_kit::assets::IconName as Lucide;
use gpui_kit::component::button::ButtonVariants as _;
use gpui_kit::component::*;
use gpui_kit::*;

use super::SettingsView;
use super::widgets::{Tone, card, chip, hashed_id, note, row, section};
use crate::shortcuts::{Command, label, normalize};
use crate::ui::kit::Hint as _;
use crate::ui::prefs;

impl SettingsView {
    pub(super) fn shortcuts_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let shortcuts = &prefs::current(cx).shortcuts;
        let rows = Command::ALL.into_iter().map(|command| {
            let recording = self
                .recording
                .as_ref()
                .is_some_and(|(it, _)| *it == command);
            let mut control = h_flex().gap_1().items_center().flex_wrap().justify_end();
            for chord in shortcuts.chords(command) {
                control = control.child(self.chord(command, chord, cx));
            }
            control = if recording {
                control
                    .child(chip("Нажмите сочетание…", Tone::Accent, cx))
                    .child(
                        kit::button(hashed_id("shortcut-cancel", &command.id()))
                            .label("Отмена")
                            .ghost()
                            .xsmall()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.recording = None;
                                cx.notify();
                            })),
                    )
            } else {
                control.child(
                    kit::button(hashed_id("shortcut-add", &command.id()))
                        .icon(IconName::Plus)
                        .ghost()
                        .xsmall()
                        .hint("Добавить сочетание", cx)
                        .on_click(
                            cx.listener(move |this, _, _, cx| this.start_recording(command, cx)),
                        ),
                )
            };
            if !shortcuts.is_default(command) {
                control = control.child(
                    kit::button(hashed_id("shortcut-reset", &command.id()))
                        .icon(Lucide::RotateCcw)
                        .ghost()
                        .xsmall()
                        .hint("Как по умолчанию", cx)
                        .on_click(move |_, _, cx| {
                            prefs::update(cx, |prefs| prefs.shortcuts.reset(command))
                        }),
                );
            }
            let hint =
                (!command.in_terminal()).then(|| "В фокусе терминала уходит в shell.".into());
            row(command.title(), hint, control, false, cx).into_any_element()
        });
        let reset_all = (!shortcuts.is_empty()).then(|| {
            kit::button("shortcuts-reset-all")
                .label("Сбросить все")
                .outline()
                .xsmall()
                .on_click(|_, _, cx| prefs::update(cx, |prefs| prefs.shortcuts.reset_all()))
                .into_any_element()
        });
        vec![
            section(
                "Команды окна",
                Some(
                    "Сочетание, занятое другой командой, переходит к новой. Esc отменяет запись."
                        .into(),
                ),
                reset_all,
                card(rows, cx),
                cx,
            )
            .into_any_element(),
            note(
                "В фокусе терминала остальные клавиши уходят в shell; команды окна срабатывают поверх него.",
                cx,
            )
            .into_any_element(),
        ]
    }

    fn chord(&self, command: Command, chord: String, cx: &Context<Self>) -> AnyElement {
        let theme = cx.theme();
        h_flex()
            .gap_0p5()
            .pl_2()
            .rounded(theme.radius)
            .border_1()
            .border_color(theme.border)
            .bg(theme.secondary)
            .text_xs()
            .font_family(theme.mono_font_family.clone())
            .child(label(&chord))
            .child(
                kit::button(hashed_id("shortcut-remove", &(command.id(), &chord)))
                    .icon(IconName::Close)
                    .ghost()
                    .xsmall()
                    .hint("Убрать", cx)
                    .on_click(move |_, _, cx| {
                        prefs::update(cx, |prefs| prefs.shortcuts.remove(command, &chord))
                    }),
            )
            .into_any_element()
    }

    fn start_recording(&mut self, command: Command, cx: &mut Context<Self>) {
        let view = cx.weak_entity();
        // Ahead of the keymap, so a recorded chord does not also run.
        let intercept = cx.intercept_keystrokes(move |event, _, cx| {
            let keystroke = event.keystroke.clone();
            if view
                .update(cx, |this, cx| this.record(&keystroke, cx))
                .unwrap_or(false)
            {
                cx.stop_propagation();
            }
        });
        self.recording = Some((command, intercept));
        cx.notify();
    }

    /// Takes the keystroke while recording; a chord needs a modifier unless
    /// it is a function key, so plain typing keys stay free.
    fn record(&mut self, keystroke: &Keystroke, cx: &mut Context<Self>) -> bool {
        let Some((command, _)) = &self.recording else {
            return false;
        };
        let command = *command;
        let modifiers = &keystroke.modifiers;
        if keystroke.key == "escape" && !modifiers.modified() {
            self.recording = None;
            cx.notify();
            return true;
        }
        let function_key = keystroke.key.len() > 1
            && keystroke.key.starts_with('f')
            && keystroke.key[1..].chars().all(|c| c.is_ascii_digit());
        if !(modifiers.control || modifiers.alt || modifiers.platform || function_key) {
            return true;
        }
        let Ok(chord) = normalize(&keystroke.unparse()) else {
            return true;
        };
        self.recording = None;
        prefs::update(cx, |prefs| {
            // `chord` is canonical already, so adding cannot fail.
            let _ = prefs.shortcuts.add(command, &chord);
        });
        cx.notify();
        true
    }
}
