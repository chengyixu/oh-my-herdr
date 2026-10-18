import { expect, test } from "bun:test";
import { nativeContextSnapshot as snapshotContext, prepareNativeContextCommand as prepareContext, startNativeContextServer as startContext } from "./herdr-agent-state";
// Plain-message fixtures need no extended-role conversion. Production receives
// convertToLlm from the running native SDK, exercised by the native verifier.
const projectForModel = (messages: any[]) => messages;
const nativeContextSnapshot = (ctx: any, ready: () => boolean, project = projectForModel) => snapshotContext(ctx, ready, project);
const prepareNativeContextCommand = (ctx: any, ready: () => boolean, request: any, token: string) => prepareContext(ctx, ready, request, token, projectForModel);
const startNativeContextServer = (ctx: any, ready: () => boolean, register: (endpoint: string, token: string) => Promise<boolean>) => startContext(ctx, ready, register, projectForModel);
import net from "node:net";
import fs from "node:fs";

function fixture() {
  let draft = "";
  let busy = false;
  let queued = false;
  let leaf = "last";
  let shutdowns = 0;
  const branch = Array.from({ length: 600 }, (_, index) => ({
    type: "message", id: `entry-${index}`, parentId: index ? `entry-${index - 1}` : null,
    message: { role: index % 2 ? "assistant" : "user", content: `complete-message-${index}` },
  }));
  branch.push({ type: "message", id: "last", parentId: "entry-599", message: {
    role: "toolResult", toolCallId: "native-call", content: [{ type: "text", text: "full-tool-result-" + "z".repeat(30_000) }],
  }} as any);
  const ctx = {
    mode: "tui", cwd: "/owned-project", isIdle: () => !busy, hasPendingMessages: () => queued,
    ui: { getEditorText: () => draft }, getSystemPrompt: () => "complete-source-system-context",
    shutdown: () => { shutdowns++; },
    sessionManager: {
      getSessionId: () => "exact-native-session", getSessionFile: () => "/owned-session.jsonl",
      getLeafId: () => leaf, getHeader: () => ({ type: "session", version: 3, id: "exact-native-session" }),
      getBranch: () => branch, buildSessionProjection: () => ({ messages: branch.map(entry => entry.message), entries: branch }),
    },
  };
  return { ctx, setDraft: (value: string) => { draft = value; }, setBusy: (value: boolean) => { busy = value; },
    setQueued: (value: boolean) => { queued = value; }, setLeaf: (value: string) => { leaf = value; }, shutdowns: () => shutdowns };
}

const token = "test-only-process-capability";

test("native export captures the exact active branch and complete context without a screen excerpt", () => {
  const state = fixture();
  const exported = nativeContextSnapshot(state.ctx, () => true);
  expect(exported.session_id).toBe("exact-native-session");
  expect(exported.leaf_id).toBe("last");
  expect(exported.branch).toHaveLength(601);
  expect(exported.branch[0].message.content).toBe("complete-message-0");
  expect(exported.branch[600].message.content[0].text).toBe("full-tool-result-" + "z".repeat(30_000));
  expect(exported.system_prompt).toBe("complete-source-system-context");
  expect(exported.projection.messages).toHaveLength(601);
  expect(state.shutdowns()).toBe(0);
});

test("native export keeps recorded history separate from the SDK's active model context", () => {
  const state = fixture();
  const messages = [
    { role: "compactionSummary", summary: "current checkpoint" },
    { role: "bashExecution", excludeFromContext: true, output: "excluded diagnostic" },
  ];
  state.ctx.sessionManager.buildSessionProjection = () => ({ messages } as any);
  const exported = nativeContextSnapshot(state.ctx, () => true, input => {
    expect(input).toBe(messages);
    return [{ role: "user", content: "native converted current checkpoint" }];
  });
  expect(exported.branch[0].message.content).toBe("complete-message-0");
  expect(exported.model_messages).toEqual([{ role: "user", content: "native converted current checkpoint" }]);
  expect(state.shutdowns()).toBe(0);
  expect(() => nativeContextSnapshot(state.ctx, () => true, () => [])).toThrow();
  messages.push({ role: "unqualified-future-role" } as any);
  expect(() => nativeContextSnapshot(state.ctx, () => true, projectForModel)).toThrow();
});

test("drafts, active work, queued messages, and native blockers reject export without altering the session", () => {
  for (const reject of ["draft", "busy", "queued", "blocker"]) {
    const state = fixture();
    if (reject === "draft") state.setDraft("unsent draft, keep every character");
    if (reject === "busy") state.setBusy(true);
    if (reject === "queued") state.setQueued(true);
    expect(() => nativeContextSnapshot(state.ctx, () => reject !== "blocker")).toThrow();
    expect(state.shutdowns()).toBe(0);
  }
});

test("closing requires the same session snapshot and executes only after the acknowledgement flushes", () => {
  const state = fixture();
  const exported = prepareNativeContextCommand(state.ctx, () => true, { token, action: "snapshot" }, token);
  const close = prepareNativeContextCommand(state.ctx, () => true, { token, action: "close", snapshot_id: exported.body.snapshot_id }, token);
  expect(close.body.closing).toBe(true);
  expect(state.shutdowns()).toBe(0);
  close.afterFlush();
  expect(state.shutdowns()).toBe(1);
});

test("wrong capability, stale branch, and changes after acknowledgement never close the source", () => {
  const state = fixture();
  const exported = prepareNativeContextCommand(state.ctx, () => true, { token, action: "snapshot" }, token);
  expect(() => prepareNativeContextCommand(state.ctx, () => true, { token: "wrong", action: "close", snapshot_id: exported.body.snapshot_id }, token)).toThrow();
  state.setLeaf("another-branch");
  expect(() => prepareNativeContextCommand(state.ctx, () => true, { token, action: "close", snapshot_id: exported.body.snapshot_id }, token)).toThrow();
  state.setLeaf("last");
  const close = prepareNativeContextCommand(state.ctx, () => true, { token, action: "close", snapshot_id: exported.body.snapshot_id }, token);
  state.setDraft("typed after the close acknowledgement");
  close.afterFlush();
  expect(state.shutdowns()).toBe(0);
});

function exchange(endpoint: string, request: unknown): Promise<any> {
  return new Promise((resolve, reject) => {
    const socket = net.createConnection(endpoint);
    let bytes = "";
    socket.setTimeout(2_000, () => socket.destroy(new Error("native context response timed out")));
    socket.on("error", reject);
    socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    socket.on("data", chunk => {
      bytes += chunk.toString("utf8");
      if (bytes.includes("\n")) { socket.destroy(); resolve(JSON.parse(bytes.split("\n")[0])); }
    });
  });
}

test.skipIf(process.platform === "win32")("native context transport keeps private permissions and closes only a verified unchanged source", async () => {
  const state = fixture();
  let capability = "";
  const service = await startNativeContextServer(state.ctx, () => true, async (_endpoint, value) => { capability = value; return true; });
  try {
    expect(fs.statSync(service.endpoint).mode & 0o777).toBe(0o600);
    const rejected = await exchange(service.endpoint, { token: "unauthorized", action: "snapshot" });
    expect(rejected.error.code).toBe("context_not_authorized");
    const exported = await exchange(service.endpoint, { token: capability, action: "snapshot" });
    expect(exported.snapshot.branch).toHaveLength(601);
    state.setDraft("keep this draft");
    const refused = await exchange(service.endpoint, { token: capability, action: "close", snapshot_id: exported.snapshot_id });
    expect(refused.error.code).toBe("source_has_draft");
    expect(state.shutdowns()).toBe(0);
    state.setDraft("");
    const accepted = await exchange(service.endpoint, { token: capability, action: "close", snapshot_id: exported.snapshot_id });
    expect(accepted.closing).toBe(true);
    expect(state.shutdowns()).toBe(1);
  } finally { await service.close(); await service.close(); }
  expect(fs.existsSync(service.endpoint)).toBe(false);
});

test.skipIf(process.platform === "win32")("a rejected server registration releases the owned native socket", async () => {
  const state = fixture();
  let endpoint = "";
  await expect(startNativeContextServer(state.ctx, () => true, async (value) => { endpoint = value; return false; })).rejects.toThrow();
  expect(fs.existsSync(endpoint)).toBe(false);
  expect(state.shutdowns()).toBe(0);
});
