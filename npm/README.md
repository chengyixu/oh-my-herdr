# OhMyHerdr

Integration-driven terminal harnesses, native agent profiles, and shared coding spaces.

```sh
npm install --global ohmyherdr

# Launch after installation
ohmyherdr
```

This package installs the version-pinned, checksum-verified native OhMyHerdr
binary for Apple Silicon macOS. OhMyHerdr has its own server, configuration,
sockets, sessions, and updater.

## What it includes

- A terminal-only catalog of 40 coding CLI tools. Activate installed tools in
  **Integrations**; harness pickers and profile controls follow that metadata.
  Tool CLIs are installed separately and retain their native authentication and
  model configuration.
- Saved agent profiles with a working directory and durable Markdown documents.
  Integrations with profile-instruction support forward those documents using
  their native launch arguments; other tools use their native context settings.
- A multiline document editor supporting `AGENTS.md` and additional named files.
- Shared Spaces. Click a saved agent to open **Use space cwd**, **Use agent native
  cwd**, **Native harness**, **Edit profile**, and **Delete profile**.
- A pane control such as `Harness: pi`. Qualified native adapters replace the
  harness in that same pane, preserving its pane ID, terminal, shell and layout.
  Pi 1.1 → Codex 0.163 currently preserves complete recorded message history,
  tool calls/results and images. Native compaction checkpoints and context
  exclusions are preserved separately from the complete recorded history.
  The source branch is retained for recovery.
  Drafts, native blockers, unsupported content or unqualified integration pairs
  stop the action before the source is closed; there is no split-pane fallback.
- Persistent current-tab, grouped, priority, and saved-profile Agents views.

## Activation

```sh
ohmyherdr integration list
ohmyherdr integration activate opencode
ohmyherdr integration deactivate opencode
```

Deactivation prevents future launches and revival without stopping current
processes or deleting saved profiles and documents. Activation is separate from
installing a tool's CLI or status hook.

## Updates

Publishing a release does not install it or restart an existing runtime. When
ready, run the updater and follow its migration guidance:

```sh
ohmyherdr update
```

Use `ohmyherdr update --handoff` when ready to install this release and hand
the running server to it. This is required to activate live harness switching;
a plain `ohmyherdr update` keeps the running server on its existing build.
Hand off only when ready to change the live server. Reopen the updated TUI client,
then install the updated Pi/Codex integrations. Run `/reload` inside each existing
Pi conversation to register its new native exporter. Saved activation data and
native replacement recovery use
registry format 6; older writers preserve newer registry files without modifying
them.

## Platform support

The npm launcher and published native artifact support Apple Silicon macOS
(`darwin` / `arm64`). See the [GitHub releases](https://github.com/chengyixu/oh-my-herdr/releases)
for the native artifact and release history.
