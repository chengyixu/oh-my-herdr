# Native same-pane source verification

The source integration exports its exact native session while its external editor
holds the empty composer. The runtime imports a separate native target session,
checks that the source did not change, sends one native exit command, and starts
the target in the original shell. Completion requires a native readiness
acknowledgement. A process name or a startup screen is insufficient.

## Qualified path

On Apple Silicon macOS, managed launches and cold restores create a private,
per-launch editor service for Codex and Claude. Pi exports through its installed
extension. The source lease is ephemeral and transferred over the authenticated
local server handoff; it is not written into a session snapshot. An abandoned
handoff cannot kill the original source's service. An adopted service is stopped
through its authenticated socket, avoiding signals to a possibly reused PID.

Integration selection, native launch/exit controls, context formats, imports,
and permission defaults remain integration-owned. The client requests the
server's `agent.harness.replace` operation.

Verified native versions: Codex 0.164.0, Claude Code 2.1.310, and Pi 1.4.0
with integration v11, native context edits and session projection support.
The tests use real native CLIs and task-owned local HTTP protocol fixtures. No
operator model, provider, account, or credential configuration is changed.
Retained-pane cold restoration is also verified with Claude Code 2.1.311.
That resumes an existing native conversation; Claude history imports retain
their separate 2.1.310 qualification gate. A newer unqualified importer version
refuses the switch before source exit.

## Reproducible checks

Run the normal local checks with `just test` and `just lint`. The native editor
service's privacy, draft, queue, changed-context and single-exit regression tests
are part of `just integration-assets-test`.

Build an Ohmyherdr product binary with `OHMYHERDR_BUILD=1 cargo build --locked`.
Run these commands with its path as `BINARY`:

```sh
python3 scripts/verify_native_harness_switch.py --binary "$BINARY" \
  --local-protocol-fixture --exercise-turns \
  --chain codex,claude,pi,codex,pi,claude,codex --output ./proof/full
python3 scripts/verify_native_harness_switch.py --binary "$BINARY" \
  --local-protocol-fixture --exercise-turns --compacted-source \
  --chain codex,claude,pi,codex,pi,claude,codex --output ./proof/compacted
python3 scripts/verify_native_harness_switch.py --binary "$BINARY" \
  --local-protocol-fixture --exercise-turns --handoff-between-switches \
  --chain codex,claude,pi --output ./proof/handoff
python3 scripts/verify_native_harness_switch.py --binary "$BINARY" \
  --local-protocol-fixture --exercise-turns --restart-between-switches \
  --chain codex,claude,pi --output ./proof/restart
```

The sequence covers Pi → Codex, Codex → Claude, Claude → Pi, Codex → Pi,
Pi → Claude, and Claude → Codex. Each transition rejects an unsent draft,
preserves it, then succeeds after native cancellation. It checks the pane,
terminal, shell PID and roster identity and both ends of the recorded history.
For Codex and Claude it also captures an actual outgoing model request from a
private copy of the imported session. For Pi it loads the session through the
native SDK and inspects `convertToLlm(buildSessionContext().messages)`.
The optional turn exercise submits real prompts to native Codex/Claude TUIs and
waits for the fixture's assistant reply before switching again.

Fresh native-created histories are verified separately, rather than assuming an
imported legacy rollout represents a new paginated Codex thread:

```sh
python3 scripts/verify_native_fresh_sources.py --binary "$BINARY" \
  --source codex --output ./proof/fresh-codex
python3 scripts/verify_native_fresh_sources.py --binary "$BINARY" \
  --source claude --output ./proof/fresh-claude
```

These create native sessions, submit real turns through the local fixture, and
then switch through all three harnesses. Native Pi model selection is checked
independently from historical assistant provenance. Native Codex queue reads use
`thread/queue/list`; no private database schema is treated as an API.

Compacted runs additionally require the active summary and retained context in
native requests, and prohibit resurrection of old messages and excluded
execution diagnostics. Complete older history must still exist in the target
file. A refusal is a failed transition, not successful pair coverage.

Retained-pane cold restore has a separate native reproduction. It terminates only task-owned native foreground process groups while retaining their shells, stops the server, verifies the saved exact references, then cold-starts the server. Fifteen Pi panes exercise background restoration alongside native Claude and Codex. Pi must render its original conversation marker in a real foreground native process; Claude and Codex must also send the original marker in their next model request to the local fixture. Saved metadata alone cannot pass this check.

```sh
python3 scripts/verify_native_cold_restore.py --binary "$BINARY" \
  --pi-panes 15 --output ./proof/retained-cold-restore
```

## Deliberate refusal boundaries

- Existing manually launched Claude/Codex processes that did not inherit the
  editor service cannot acquire a different `EDITOR` environment in place. Their
  sessions must be resumed through a managed launch once. Installing the binary
  alone cannot change an already running process's environment.
- Paginated Codex histories are read through native `thread/items/list` until
  `nextCursor` is explicitly null, then archived with the complete native export.
  The rollout supplies the distinct active model window; older native pages must
  not be expanded back into it. Opaque encrypted reasoning, unknown
  content, ambiguous branches, incomplete files, changed identities and unknown
  attachment/key sequences are refused before source exit.
- Pending native queues are refused. Completed ordinary prompts are allowed
  after native queue checks and an editor-held snapshot. Queue inspection is
  scoped to the exact session; sibling queues do not claim the pane.
- Target account errors, trust dialogs and native confirmations must be resolved
  in the native harness. Codex's read-only account/bootstrap check runs before
  source export/exit, with the profile's process-local credential binding. A known
  account failure therefore retains the original process and unsent draft.
  They do not count as a ready target. No credentials or trust settings are changed.
- Empty-history Claude imports are refused before shutdown until a native empty
  session encoding is qualified; an empty transcript is not silently published.
- Other activated harnesses are not qualified as switch sources by this change.
  Their menu presence or a target import adapter is not an end-to-end result.

These boundaries must be reflected in release claims. The earlier statement
that every activated harness already supported lossless switching was incorrect.
