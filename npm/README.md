# OhMyHerdr

Native agent profiles and shared coding spaces for Codex, Pi, and Claude Code.

```sh
npm install --global ohmyherdr
ohmyherdr
```

This package installs the checksum-verified native OhMyHerdr binary for Apple
Silicon macOS. It is a separate product runtime: it does not share or replace a
stable Herdr installation.

## What it includes

- Native Codex, Pi, and Claude Code profiles with their own working directory
  and durable `AGENTS.md`.
- A readable, multiline profile document editor. Profiles can also own named
  Markdown documents alongside `AGENTS.md`.
- Shared Spaces where each spawned agent can use either the Space directory or
  its profile's native directory, while retaining its own instructions.
- A visual Agents-sidebar workflow to create, inspect, edit, spawn, and delete
  profiles without hardcoded roles or duplicate profile rows.
- An isolated server, configuration, sockets, sessions, and OTA updater that
  never conflicts with stable Herdr.

## Updates

After installation, update the native product independently:

```sh
ohmyherdr update --handoff
```

The updater verifies the public release checksum and hands running panes to the
new product server when the release supports live handoff.

## Platform support

The first NPM release supports Apple Silicon macOS (`darwin` / `arm64`) only.
See the [GitHub releases](https://github.com/chengyixu/oh-my-herdr/releases)
for the native artifact and release history.
