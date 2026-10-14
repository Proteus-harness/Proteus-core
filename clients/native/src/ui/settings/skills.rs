//! Skills page: the providers' skill catalogs, switched by
//! `addons.disabled_skills` of the draft.

use std::collections::BTreeSet;

use gpui_kit::component::*;
use gpui_kit::prelude::FluentBuilder as _;
use gpui_kit::*;
use proteus_contracts::domain::SkillDescriptor;

use super::SettingsView;
use super::widgets::{Tone, card, changed_dot, chip, hashed_id, mono, note, section};
use crate::settings::addons::{missing_skills, package_of, skills};
use crate::ui::kit;
use crate::ui::tip::Tip as _;

impl SettingsView {
    pub(super) fn skills_page(&self, cx: &Context<Self>) -> Vec<AnyElement> {
        let mut parts: Vec<AnyElement> = self.addons_notice(cx).into_iter().collect();
        let Some(addons) = self.addons() else {
            return parts;
        };
        let problems: Vec<AnyElement> = addons
            .catalogs
            .iter()
            .flat_map(|catalog| {
                let provider = catalog.provider.as_str();
                catalog
                    .error
                    .iter()
                    .map(move |error| (Tone::Danger, format!("{provider}: {error}")))
                    .chain(
                        catalog
                            .catalog
                            .iter()
                            .flat_map(|catalog| &catalog.warnings)
                            .map(move |warning| (Tone::Warning, format!("{provider}: {warning}"))),
                    )
            })
            .map(|(tone, text)| problem(text, tone, cx))
            .collect();
        if !problems.is_empty() {
            parts.push(card(problems, cx).into_any_element());
        }
        let skills = skills(addons);
        let providers: BTreeSet<&str> = skills.iter().map(|(provider, _)| *provider).collect();
        let enabled = skills
            .iter()
            .filter(|(_, skill)| self.draft.skill_enabled(&skill.id))
            .count();
        let mut rows: Vec<AnyElement> = skills
            .iter()
            .map(|(provider, skill)| {
                self.skill_row(skill, (providers.len() > 1).then_some(*provider), cx)
            })
            .collect();
        rows.extend(
            missing_skills(&self.draft, addons)
                .into_iter()
                .map(|id| self.missing_skill_row(id, cx)),
        );
        if rows.is_empty() {
            rows.push(note("Поставщики контекста не нашли навыков.", cx).into_any_element());
        }
        parts.push(
            section(
                format!("Навыки · {enabled} из {}", skills.len()),
                Some("Изменения применяются после сохранения.".into()),
                None,
                card(rows, cx),
                cx,
            )
            .into_any_element(),
        );
        parts
    }

    fn skill_row(
        &self,
        skill: &SkillDescriptor,
        provider: Option<&str>,
        cx: &Context<Self>,
    ) -> AnyElement {
        let theme = cx.theme();
        let on = self.draft.skill_enabled(&skill.id);
        let changed = on != self.baseline.skill_enabled(&skill.id);
        let mut chips = vec![chip(skill.source.clone(), Tone::Muted, cx)];
        chips.extend(provider.map(|provider| chip(provider.to_string(), Tone::Muted, cx)));
        // Off in the saved catalog though the list allows it: its package is off.
        if !skill.enabled && self.baseline.skill_enabled(&skill.id) {
            let package = self
                .addons()
                .and_then(|addons| package_of(addons, &skill.id));
            chips.push(chip(
                package.map_or("не загружается".to_string(), |package| {
                    format!("пакет {package} выключен")
                }),
                Tone::Warning,
                cx,
            ));
        }
        let id = skill.id.clone();
        h_flex()
            .px_4()
            .py_3()
            .gap_4()
            .items_start()
            .child(
                v_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_1()
                    .child(
                        h_flex()
                            .id(hashed_id("skill-name", &skill.id))
                            .gap_2()
                            .items_center()
                            .child(mono(skill.id.clone(), cx).text_sm())
                            .when(changed, |line| line.child(changed_dot(cx)))
                            .tip_text(skill.path.display().to_string()),
                    )
                    .when(!skill.description.trim().is_empty(), |text| {
                        text.child(
                            div()
                                .text_xs()
                                .text_color(theme.muted_foreground)
                                .child(skill.description.trim().to_string()),
                        )
                    })
                    .child(h_flex().flex_wrap().gap_1().children(chips)),
            )
            .child(
                div().pt_0p5().child(
                    kit::switch(hashed_id("skill", &skill.id))
                        .checked(on)
                        .on_click(cx.listener(move |this, checked: &bool, _, cx| {
                            let checked = *checked;
                            this.update_draft(cx, |draft| draft.set_skill(&id, checked));
                        })),
                ),
            )
            .into_any_element()
    }

    /// Disabled entry that no catalog lists any more; switching it on only
    /// removes the stale entry.
    fn missing_skill_row(&self, id: &str, cx: &Context<Self>) -> AnyElement {
        let target = id.to_string();
        h_flex()
            .px_4()
            .py_3()
            .gap_4()
            .items_center()
            .child(
                h_flex()
                    .flex_1()
                    .min_w_0()
                    .gap_2()
                    .items_center()
                    .child(mono(id.to_string(), cx).text_sm())
                    .child(chip("нет в каталоге", Tone::Warning, cx)),
            )
            .child(
                kit::switch(hashed_id("missing-skill", &id))
                    .checked(false)
                    .on_click(cx.listener(move |this, _: &bool, _, cx| {
                        this.update_draft(cx, |draft| draft.set_skill(&target, true));
                    })),
            )
            .into_any_element()
    }
}

fn problem(text: String, tone: Tone, cx: &App) -> AnyElement {
    let theme = cx.theme();
    note(text, cx)
        .text_color(match tone {
            Tone::Danger => theme.danger,
            _ => theme.warning,
        })
        .into_any_element()
}
