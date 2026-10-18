//! Integration-owned terminal harness contracts. Provider configuration stays
//! with each native harness and Magpie. Desktop apps and browser adapters are
//! intentionally excluded.
use crate::api::schema::IntegrationTarget;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Instructions {
    Codex,
    AppendFiles(&'static str),
    AppendText(&'static str),
    Prompt,
    Native,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Effort {
    None,
    Flag,
    CodexConfig,
    ModelSuffix,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tools {
    RepeatedFlag,
    CommaSeparated,
}
#[derive(Debug, Clone, Copy)]
pub struct Controls {
    pub model: bool,
    pub effort: Effort,
    pub key_env: Option<&'static str>,
    pub tools: Option<Tools>,
}
const NATIVE: Controls = Controls {
    model: false,
    effort: Effort::None,
    key_env: None,
    tools: None,
};
const MODEL: Controls = Controls {
    model: true,
    ..NATIVE
};
const CODEX: Controls = Controls {
    model: true,
    effort: Effort::CodexConfig,
    key_env: Some("OPENAI_API_KEY"),
    tools: None,
};
const CLAUDE: Controls = Controls {
    model: true,
    effort: Effort::Flag,
    key_env: Some("ANTHROPIC_API_KEY"),
    tools: Some(Tools::RepeatedFlag),
};
const PI: Controls = Controls {
    model: true,
    effort: Effort::ModelSuffix,
    key_env: None,
    tools: Some(Tools::CommaSeparated),
};

#[derive(Debug)]
pub struct Harness {
    pub agent: Agent,
    pub id: &'static str,
    pub name: &'static str,
    pub executable: &'static str,
    pub launch_args: &'static [&'static str],
    pub instructions: Instructions,
    pub controls: Controls,
    pub hook: Option<IntegrationTarget>,
    pub installer: Option<&'static str>,
}
macro_rules! terminal_harnesses {
    ($($variant:ident => ($id:literal, $name:literal, $exe:expr, $args:expr, $instructions:expr, $controls:expr, $hook:expr, $installer:expr)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        #[repr(usize)]
        pub enum Agent { $($variant),+ }
        impl Agent {
            pub const ALL: [Self; [$(terminal_harnesses!(@unit $variant)),+].len()] = [$(Self::$variant),+];
            pub const fn harness(self) -> &'static Harness { &HARNESSES[self as usize] }
        }
        pub const HARNESSES: &[Harness] = &[$(Harness {
            agent: Agent::$variant, id: $id, name: $name, executable: $exe,
            launch_args: $args, instructions: $instructions, controls: $controls,
            hook: $hook, installer: $installer,
        }),+];
    };
    (@unit $variant:ident) => { () };
}
use Instructions::{AppendFiles, AppendText, Codex, Native, Prompt};
use IntegrationTarget as Hook;
terminal_harnesses! {
    Codex => ("codex", "Codex", "codex", &["--no-daemon"], Codex, CODEX, Some(Hook::Codex), Some("npm install -g @openai/codex")),
    Pi => ("pi", "Pi", "pi", &[], AppendFiles("--append-system-prompt"), PI, Some(Hook::Pi), Some("npm install -g @earendil-works/pi-coding-agent")),
    Claude => ("claude", "Claude Code", "claude", &[], AppendFiles("--append-system-prompt-file"), CLAUDE, Some(Hook::Claude), Some("npm install -g @anthropic-ai/claude-code")),
    Omp => ("omp", "Oh My Pi", "omp", &[], AppendFiles("--append-system-prompt"), MODEL, Some(Hook::Omp), Some("npm install -g @oh-my-pi/pi-coding-agent")),
    OpenCode => ("opencode", "OpenCode", "opencode", &[], Prompt, MODEL, Some(Hook::Opencode), Some("npm install -g opencode-ai")),
    GithubCopilot => ("copilot", "GitHub Copilot", "copilot", &[], Native, MODEL, Some(Hook::Copilot), Some("npm install -g @github/copilot")),
    Cursor => ("cursor", "Cursor Agent", if cfg!(windows) { "cursor-agent.cmd" } else { "cursor-agent" }, &[], Native, NATIVE, Some(Hook::Cursor), None),
    Gemini => ("gemini", "Gemini CLI", "gemini", &[], Native, NATIVE, None, Some("npm install -g @google/gemini-cli")),
    Antigravity => ("agy", "Antigravity CLI", "agy", &[], Native, NATIVE, Some(Hook::AntigravityCli), None),
    Cline => ("cline", "Cline CLI", "cline", &[], Native, NATIVE, None, Some("npm install -g cline")),
    Devin => ("devin", "Devin CLI", "devin", &[], Native, NATIVE, Some(Hook::Devin), None),
    Droid => ("droid", "Factory Droid", "droid", &[], Native, NATIVE, Some(Hook::Droid), None),
    Kimi => ("kimi", "Kimi Code", "kimi", &[], Native, NATIVE, Some(Hook::Kimi), Some("uv tool install kimi-cli")),
    Kiro => ("kiro", "Kiro CLI", "kiro-cli", &[], Native, NATIVE, None, None),
    Amp => ("amp", "Amp", "amp", &[], Native, NATIVE, None, Some("npm install -g @ampcode/cli")),
    Grok => ("grok", "Grok Build", "grok", &[], Native, NATIVE, Some(Hook::Grok), None),
    Hermes => ("hermes", "Hermes Agent", "hermes", &[], Native, NATIVE, Some(Hook::Hermes), None),
    Kilo => ("kilo", "Kilo CLI", "kilo", &[], Prompt, MODEL, Some(Hook::Kilo), Some("npm install -g @kilocode/cli")),
    Qodercli => ("qodercli", "Qoder CLI", "qodercli", &[], AppendText("--append-system-prompt"), MODEL, Some(Hook::Qodercli), Some("npm install -g @qoder-ai/qodercli")),
    QoderCN => ("qoderclicn", "Qoder CLI China", "qoderclicn", &[], Native, NATIVE, None, None),
    Qwen => ("qwen", "Qwen Code", "qwen", &[], AppendText("--append-system-prompt"), MODEL, Some(Hook::Qwen), Some("npm install -g @qwen-code/qwen-code")),
    Letta => ("letta", "Letta Code", "letta", &[], Native, MODEL, None, Some("npm install -g @letta-ai/letta-code")),
    Mastracode => ("mastracode", "Mastra Code", "mastracode", &[], Native, NATIVE, Some(Hook::Mastracode), Some("npm install -g mastracode")),
    Maki => ("maki", "Maki", "maki", &[], Native, NATIVE, None, None),
    Muse => ("muse", "Muse", "muse", &[], Native, NATIVE, None, None),
    MiMo => ("mimocode", "MiMo Code", "mimo", &[], Prompt, MODEL, None, Some("npm install -g @mimo-ai/cli")),
    StepCode => ("stepcode", "StepCode", "step", &[], Native, NATIVE, None, None),
    Omo => ("omo", "OmO", "omo", &[], AppendFiles("--append-system-prompt"), MODEL, None, Some("npm install -g omo-ai")),
    Prime => ("prime-agent", "Prime Agent", "prime-agent", &[], Native, NATIVE, None, None),
    Goose => ("goose", "Goose", "goose", &["session"], Native, NATIVE, None, Some("brew install block-goose-cli")),
    Crush => ("crush", "Crush", "crush", &[], Native, NATIVE, None, Some("npm install -g @charmland/crush")),
    Dsh => ("dsh", "DeepSeek Harness", "dsh", &["tui"], Native, NATIVE, None, Some("npm install -g @deepseek-ai/dsh")),
    CommandCode => ("commandcode", "Command Code", "command-code", &[], Native, NATIVE, None, Some("npm install -g command-code")),
    Empryo => ("empryo", "Empryo", "empryo", &[], Native, NATIVE, None, None),
    Ante => ("ante", "Ante", "ante", &[], Native, NATIVE, None, None),
    Jcode => ("jcode", "jcode", "jcode", &[], Native, NATIVE, None, None),
    MiniMax => ("minimax-code", "MiniMax Code", "mcode", &[], Native, NATIVE, None, Some("npm install -g @minimax-ai/code")),
    Codebuddy => ("codebuddy", "CodeBuddy Code", "codebuddy", &[], AppendText("--append-system-prompt"), MODEL, None, Some("npm install -g @tencent-ai/codebuddy-code")),
    Atomcode => ("atomcode", "AtomCode", "atomcode", &[], Native, NATIVE, None, Some("npm install -g @atomgit.com/atomcode")),
    Snow => ("snow", "Snow CLI", "snow", &[], Native, NATIVE, None, Some("npm install -g snow-ai")),
}

pub fn find(id: &str) -> Option<&'static Harness> {
    HARNESSES.iter().find(|h| h.id == id || h.executable == id)
}
impl Harness {
    pub fn default_harness() -> &'static str {
        HARNESSES.first().map_or("", |h| h.id)
    }
    /// Native runtime APIs qualified for an atomic active-branch snapshot and close.
    pub fn native_context_version(&self) -> Option<u32> {
        match self.agent {
            Agent::Pi if cfg!(unix) => Some(1),
            _ => None,
        }
    }
    pub fn native_context_format(&self) -> Option<&'static str> {
        self.native_context_version().map(|_| "pi.session.v3")
    }
    pub fn native_import_formats(&self) -> &'static [&'static str] {
        match self.agent {
            Agent::Codex => &["pi.session.v3"],
            _ => &[],
        }
    }
    /// Native import adapters qualified with the installed tool, not UI labels.
    pub fn context_import_from(&self, source_format: &str) -> bool {
        self.native_import_formats().contains(&source_format)
    }
    pub(crate) fn import_native_context(
        &self,
        snapshot: &super::native_context::NativeContextSnapshot,
    ) -> std::io::Result<super::session_history::ImportedHistory> {
        if !self.context_import_from(&snapshot.format) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "complete native history import is not qualified for this pair",
            ));
        }
        let history = super::session_history::read_pi_snapshot(snapshot)?;
        match self.agent {
            Agent::Codex => {
                super::session_history::verify_codex_import_version(self.executable)?;
                let provider = super::session_history::effective_codex_provider(
                    self.executable,
                    &snapshot.cwd,
                )?;
                super::session_history::import_codex_history(
                    &history,
                    &snapshot.cwd,
                    &super::env::codex_dir()?,
                    &provider,
                )
            }
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "target native history adapter unavailable",
            )),
        }
    }
    pub fn available(&self) -> bool {
        super::registry::command_available(self.executable)
    }
    pub fn active(
        &self,
        registry: &crate::agent_registry::AgentRegistry,
        hook_installed: bool,
    ) -> bool {
        registry
            .harness_activations
            .get(self.id)
            .copied()
            .unwrap_or_else(|| {
                hook_installed || registry.profiles.values().any(|p| p.harness == self.id)
            })
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn integration_catalog_is_complete_and_excludes_desktop_and_browser_tools() {
        let mut ids = std::collections::HashSet::new();
        for agent in Agent::ALL {
            let h = agent.harness();
            assert!(ids.insert(h.id));
            assert_eq!(crate::detect::parse_agent_label(h.id), Some(agent));
            assert_eq!(crate::detect::parse_agent_label(h.executable), Some(agent));
        }
        assert_eq!(ids.len(), HARNESSES.len());
        for excluded in [
            "claude-desktop",
            "zed",
            "vscode",
            "workbuddy",
            "zcode",
            "fx",
            "morph",
            "reasonix",
            "aside",
        ] {
            assert!(find(excluded).is_none());
        }
    }
    #[test]
    fn codex_pane_launch_and_resume_use_process_local_native_runtime() {
        let harness = find("codex").unwrap();
        assert!(harness.launch_args.contains(&"--no-daemon"));
        let plan = crate::agent_resume::plan(
            "herdr:codex",
            "codex",
            &crate::agent_resume::AgentSessionRef::id("native-id").unwrap(),
        )
        .unwrap();
        assert_eq!(plan.argv, ["codex", "--no-daemon", "resume", "native-id"]);
    }

    #[test]
    fn explicit_deactivation_wins_over_installed_hooks_and_saved_profiles() {
        let h = find("opencode").unwrap();
        let mut registry = crate::agent_registry::AgentRegistry::new();
        assert!(!h.active(&registry, false));
        assert!(h.active(&registry, true));
        registry.harness_activations.insert(h.id.into(), false);
        assert!(!h.active(&registry, true));
        registry.harness_activations.insert(h.id.into(), true);
        assert!(h.active(&registry, false));
    }
}
