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

- A terminal-only catalog of coding CLI tools. Activate installed tools in
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
  Native qualification covers all six directions among Pi 1.4.0, Codex 0.164.0
  and Claude Code 2.1.310, including fresh native-created histories and
  completed model turns through local protocol fixtures. Codex paginated history
  is collected through its native API. Compacted context remains separate from
  the complete recorded history; the source is archived for recovery.
  These checks establish native transfer behavior, not the health of your account login.
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

Use `ohmyherdr update --handoff` when installing a newer binary and activating
its server-side changes. If npm already installed the current client while your
server is older, run the native handoff explicitly from an ordinary terminal:

```sh
ohmyherdr status
ohmyherdr server live-handoff
```

The updated CLI selects its own native executable, including when invoked
through the npm wrapper. This preserves pane processes; a plain client detach
leaves the server running. Live handoff remains experimental.

Install the updated Pi integration (`ohmyherdr integration install pi`), then run
`/reload` in old Pi conversations when needed. New managed Claude/Codex launches
and cold restores receive the native editor service. An existing process that
never inherited it needs one managed resume of its exact session; changing the
installed binary cannot change that process's environment.

Other activated harnesses are not qualified as switch sources by this release.
Unsupported or opaque content and empty-history Claude imports fail before source
exit. Resolve native authentication and confirmations in the selected harness;
Codex's startup account check retains the original process if bootstrap fails.
Saved activation data and
native replacement recovery use
registry format 6; older writers preserve newer registry files without modifying
them.

## Platform support

The npm launcher and published native artifact support Apple Silicon macOS
(`darwin` / `arm64`). See the [GitHub releases](https://github.com/chengyixu/oh-my-herdr/releases)
for the native artifact and release history.
