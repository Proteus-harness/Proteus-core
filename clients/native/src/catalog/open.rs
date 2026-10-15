//! Public Agent Skills repositories offered in the sources dialog. They are
//! ordinary sources: nothing is downloaded until the user adds one.

use super::Source;

pub struct Open {
    pub title: &'static str,
    pub url: &'static str,
    pub about: &'static str,
}

impl Open {
    pub fn source(&self) -> Source {
        Source {
            url: self.url.to_string(),
            branch: None,
        }
    }
}

pub const OPEN: [Open; 4] = [
    Open {
        title: "Anthropic",
        url: "https://github.com/anthropics/skills",
        about: "docx, pdf, pptx, xlsx, frontend-design, mcp-builder, skill-creator",
    },
    Open {
        title: "OpenAI",
        url: "https://github.com/openai/skills",
        about: "Curated skills Codex: GitHub, Figma, Playwright, Sentry, деплой",
    },
    Open {
        title: "Superpowers",
        url: "https://github.com/obra/superpowers",
        about: "Планы, TDD, отладка, code review, git worktrees",
    },
    Open {
        title: "Hugging Face",
        url: "https://github.com/huggingface/skills",
        about: "Hub, датасеты, обучение моделей, Spaces",
    },
];
