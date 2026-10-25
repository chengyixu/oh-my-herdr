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
    Codex => ("codex", "Codex", "codex", &["--no-daemon", "--dangerously-bypass-approvals-and-sandbox"], Codex, CODEX, Some(Hook::Codex), Some("npm install -g @openai/codex")),
    Pi => ("pi", "Pi", "pi", &["--approve"], AppendFiles("--append-system-prompt"), PI, Some(Hook::Pi), Some("npm install -g @earendil-works/pi-coding-agent")),
    Claude => ("claude", "Claude Code", "claude", &["--dangerously-skip-permissions"], AppendFiles("--append-system-prompt-file"), CLAUDE, Some(Hook::Claude), Some("npm install -g @anthropic-ai/claude-code")),
    Omp => ("omp", "Oh My Pi", "omp", &["--auto-approve"], AppendFiles("--append-system-prompt"), MODEL, Some(Hook::Omp), Some("npm install -g @oh-my-pi/pi-coding-agent")),
    OpenCode => ("opencode", "OpenCode", "opencode", &["--auto"], Prompt, MODEL, Some(Hook::Opencode), Some("npm install -g opencode-ai")),
    GithubCopilot => ("copilot", "GitHub Copilot", "copilot", &["--yolo"], Native, MODEL, Some(Hook::Copilot), Some("npm install -g @github/copilot")),
    Cursor => ("cursor", "Cursor Agent", if cfg!(windows) { "cursor-agent.cmd" } else { "cursor-agent" }, &["--force", "--sandbox", "disabled"], Native, NATIVE, Some(Hook::Cursor), None),
    Gemini => ("gemini", "Gemini CLI", "gemini", &["--yolo"], Native, NATIVE, None, Some("npm install -g @google/gemini-cli")),
    Antigravity => ("agy", "Antigravity CLI", "agy", &["--dangerously-skip-permissions"], Native, NATIVE, Some(Hook::AntigravityCli), None),
    Cline => ("cline", "Cline CLI", "cline", &["--auto-approve", "true"], Native, NATIVE, None, Some("npm install -g cline")),
    Devin => ("devin", "Devin CLI", "devin", &["--permission-mode", "dangerous"], Native, NATIVE, Some(Hook::Devin), None),
    Droid => ("droid", "Factory Droid", "droid", &["--auto", "high"], Native, NATIVE, Some(Hook::Droid), None),
    Kimi => ("kimi", "Kimi Code", "kimi", &["--yolo"], Native, NATIVE, Some(Hook::Kimi), Some("uv tool install kimi-cli")),
    Kiro => ("kiro", "Kiro CLI", "kiro-cli", &["chat", "--trust-all-tools"], Native, NATIVE, None, None),
    Amp => ("amp", "Amp", "amp", &["--dangerously-allow-all"], Native, NATIVE, None, Some("npm install -g @ampcode/cli")),
    Grok => ("grok", "Grok Build", "grok", &["--always-approve"], Native, NATIVE, Some(Hook::Grok), None),
    Hermes => ("hermes", "Hermes Agent", "hermes", &["--yolo"], Native, NATIVE, Some(Hook::Hermes), None),
    Kilo => ("kilo", "Kilo CLI", "kilo", &["--auto"], Prompt, MODEL, Some(Hook::Kilo), Some("npm install -g @kilocode/cli")),
    Qodercli => ("qodercli", "Qoder CLI", "qodercli", &["--dangerously-skip-permissions"], AppendText("--append-system-prompt"), MODEL, Some(Hook::Qodercli), Some("npm install -g @qoder-ai/qodercli")),
    QoderCN => ("qoderclicn", "Qoder CLI China", "qoderclicn", &["--dangerously-skip-permissions"], Native, NATIVE, None, None),
    Qwen => ("qwen", "Qwen Code", "qwen", &["--yolo"], AppendText("--append-system-prompt"), MODEL, Some(Hook::Qwen), Some("npm install -g @qwen-code/qwen-code")),
    Letta => ("letta", "Letta Code", "letta", &[], Native, MODEL, None, Some("npm install -g @letta-ai/letta-code")),
    Mastracode => ("mastracode", "Mastra Code", "mastracode", &["--permission-mode", "auto"], Native, NATIVE, Some(Hook::Mastracode), Some("npm install -g mastracode")),
    Maki => ("maki", "Maki", "maki", &["--yolo"], Native, NATIVE, None, None),
    Muse => ("muse", "Muse", "muse", &["--yolo"], Native, NATIVE, None, None),
    MiMo => ("mimocode", "MiMo Code", "mimo", &["--dangerously-skip-permissions"], Prompt, MODEL, None, Some("npm install -g @mimo-ai/cli")),
    StepCode => ("stepcode", "StepCode", "step", &["--approval-mode", "auto"], Native, NATIVE, None, None),
    Omo => ("omo", "OmO", "omo", &["--permission-preset", "full-access"], AppendFiles("--append-system-prompt"), MODEL, None, Some("npm install -g omo-ai")),
    Prime => ("prime-agent", "Prime Agent", "prime-agent", &[], Native, NATIVE, None, None),
    Goose => ("goose", "Goose", "goose", &["session"], Native, NATIVE, None, Some("brew install block-goose-cli")),
    Crush => ("crush", "Crush", "crush", &["--yolo"], Native, NATIVE, None, Some("npm install -g @charmland/crush")),
    Dsh => ("dsh", "DeepSeek Harness", "dsh", &["tui"], Native, NATIVE, None, Some("npm install -g @deepseek-ai/dsh")),
    CommandCode => ("commandcode", "Command Code", "command-code", &["--yolo"], Native, NATIVE, None, Some("npm install -g command-code")),
    Empryo => ("empryo", "Empryo", "empryo", &[], Native, NATIVE, None, None),
    Ante => ("ante", "Ante", "ante", &["--yolo"], Native, NATIVE, None, None),
    Jcode => ("jcode", "jcode", "jcode", &[], Native, NATIVE, None, None),
    Codebuddy => ("codebuddy", "CodeBuddy Code", "codebuddy", &["--dangerously-skip-permissions"], AppendText("--append-system-prompt"), MODEL, None, Some("npm install -g @tencent-ai/codebuddy-code")),
    Atomcode => ("atomcode", "AtomCode", "atomcode", &["--dangerously-skip-permissions"], Native, NATIVE, None, Some("npm install -g @atomgit.com/atomcode")),
    Snow => ("snow", "Snow CLI", "snow", &["--yolo"], Native, NATIVE, None, Some("npm install -g snow-ai")),
}

pub fn find(id: &str) -> Option<&'static Harness> {
    HARNESSES.iter().find(|h| h.id == id || h.executable == id)
}
impl Harness {
    /// Merge native defaults by option group. Explicit caller values keep their
    /// authority, and already reported flags are not duplicated on cold restore.
    pub(crate) fn launch_argv(&self, args: &[String]) -> Vec<String> {
        let mut argv = vec![self.executable.to_string()];
        let mut defaults = self.launch_args.iter().peekable();
        while let Some(flag) = defaults.next() {
            let value = if flag.starts_with('-')
                && defaults.peek().is_some_and(|next| !next.starts_with('-'))
            {
                defaults.next().copied()
            } else {
                None
            };
            let provided = args
                .iter()
                .take_while(|arg| arg.as_str() != "--")
                .any(|arg| {
                    arg == flag
                        || arg
                            .strip_prefix(flag)
                            .is_some_and(|suffix| suffix.starts_with('='))
                });
            if !provided {
                argv.push((*flag).into());
                if let Some(value) = value {
                    argv.push(value.into());
                }
            }
        }
        argv.extend_from_slice(args);
        argv
    }

    /// Process-local permission settings for native CLIs that have no flag.
    pub(crate) fn launch_environment(&self) -> &'static [(&'static str, &'static str)] {
        match self.agent {
            Agent::Goose => &[("GOOSE_MODE", "auto")],
            _ => &[],
        }
    }

    /// Scope native environment defaults to the launched process. The helper
    /// execs on Unix, retaining the pane's process and shell ownership.
    pub(crate) fn scope_launch_environment(
        &self,
        argv: Vec<String>,
    ) -> std::io::Result<Vec<String>> {
        if self.launch_environment().is_empty() {
            return Ok(argv);
        }
        let mut scoped = vec![
            crate::platform::launch_executable()?
                .to_string_lossy()
                .into_owned(),
            "__harness-launch".into(),
            self.id.into(),
        ];
        scoped.extend(argv.into_iter().skip(1));
        Ok(scoped)
    }

    pub fn default_harness() -> &'static str {
        HARNESSES.first().map_or("", |h| h.id)
    }
    /// Native runtime APIs qualified for an atomic active-branch snapshot and close.
    pub fn native_context_version(&self) -> Option<u32> {
        match self.agent {
            Agent::Pi | Agent::Codex | Agent::Claude if cfg!(unix) => Some(1),
            _ => None,
        }
    }
    pub fn native_context_format(&self) -> Option<&'static str> {
        self.native_context_version().and(match self.agent {
            Agent::Pi => Some("pi.session.v3"),
            Agent::Codex => Some("codex.rollout.v163"),
            Agent::Claude => Some("claude.transcript.v2.1.302"),
            _ => None,
        })
    }
    pub fn native_editor_controls(&self) -> Option<(&'static str, &'static str)> {
        match self.agent {
            Agent::Codex => Some(("\x07", "\x04")),
            Agent::Claude => Some(("\x07", "/exit\r")),
            _ => None,
        }
    }
    pub fn native_import_formats(&self) -> &'static [&'static str] {
        match self.agent {
            Agent::Codex
            | Agent::Claude
            | Agent::Pi
            | Agent::Grok
            | Agent::OpenCode
            | Agent::Kilo => {
                &[
                    "pi.session.v3",
                    "codex.rollout.v163",
                    "claude.transcript.v2.1.302",
                ]
            }
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
        let history = super::session_history::read_native_snapshot(snapshot)?;
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
            Agent::Claude => {
                super::session_history::claude::import_claude_history(
                    &history,
                    &snapshot.cwd,
                    &super::env::claude_dir()?,
                )
            }
            Agent::Pi => {
                super::session_history::pi::import_pi_history(
                    &history,
                    &snapshot.cwd,
                    &super::env::pi_dir()?,
                )
            }
            Agent::Grok => {
                super::session_history::grok::import_grok_history(
                    &history,
                    &snapshot.cwd,
                    &super::env::grok_dir()?,
                )
            }
            Agent::OpenCode | Agent::Kilo => {
                super::session_history::opencode_family::import_family_history(
                    &history,
                    &snapshot.cwd,
                    self.executable,
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
    fn retired_terminal_harness_is_not_detected_or_launchable() {
        for label in ["minimax-code", "mcode"] {
            assert!(find(label).is_none(), "retired harness {label} is offered");
            assert!(crate::detect::parse_agent_label(label).is_none());
        }
    }

    #[test]
    fn every_catalog_member_declares_a_native_permission_default() {
        let flags = [
            "--dangerously-bypass-approvals-and-sandbox",
            "--dangerously-skip-permissions",
            "--approve",
            "--auto-approve",
            "--auto",
            "--yolo",
            "--force",
            "--permission-mode",
            "--trust-all-tools",
            "--dangerously-allow-all",
            "--always-approve",
            "--approval-mode",
            "--permission-preset",
        ];
        for harness in HARNESSES {
            let native_only = matches!(
                harness.agent,
                Agent::Letta | Agent::Prime | Agent::Empryo | Agent::Jcode | Agent::Dsh
            );
            assert!(
                native_only
                    || !harness.launch_environment().is_empty()
                    || harness.launch_args.iter().any(|arg| flags.contains(arg)),
                "{} has no reviewed permission default",
                harness.id
            );
        }
    }

    #[test]
    fn explicit_native_option_groups_are_not_duplicated_or_rewritten() {
        let devin = find("devin").unwrap();
        assert_eq!(
            devin.launch_argv(&[
                "--permission-mode".into(),
                "auto".into(),
                "--model".into(),
                "m".into()
            ]),
            ["devin", "--permission-mode", "auto", "--model", "m"]
        );
        let codex = find("codex").unwrap();
        let launch = codex.launch_argv(&["--no-daemon".into(), "resume".into(), "id".into()]);
        assert_eq!(
            launch
                .iter()
                .filter(|a| a.as_str() == "--no-daemon")
                .count(),
            1
        );
        assert!(launch.contains(&"--dangerously-bypass-approvals-and-sandbox".into()));
        assert!(launch.ends_with(&["resume".into(), "id".into()]));
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
        assert_eq!(
            plan.argv,
            [
                "codex",
                "--no-daemon",
                "--dangerously-bypass-approvals-and-sandbox",
                "resume",
                "native-id"
            ]
        );
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
