// installed by herdr
// managed by herdr; reinstalling or updating the integration overwrites this file.
// add custom hooks/plugins beside this file instead of editing it.
// HERDR_INTEGRATION_ID=pi
// HERDR_INTEGRATION_VERSION=11
// @ts-nocheck

import net from "node:net";
import path from "node:path";
import crypto from "node:crypto";
import fs from "node:fs";

class NativeContextError extends Error {
  constructor(public code: string, message: string) { super(message); }
}

type ModelProjection = (messages: any[]) => any[];

/** Snapshot the native active branch, never a terminal view or latest-by-cwd file. */
export function nativeContextSnapshot(ctx: any, canExit: () => boolean, projectForModel: ModelProjection) {
  const manager = ctx?.sessionManager;
  if (ctx?.mode !== "tui" || !manager ||
      [ctx?.isIdle, ctx?.hasPendingMessages, ctx?.shutdown, ctx?.getSystemPrompt,
       ctx?.ui?.getEditorText, manager.getHeader, manager.getBranch,
       manager.getSessionId, manager.getSessionFile, manager.getLeafId,
       manager.buildSessionProjection].some(method => typeof method !== "function")) {
    throw new NativeContextError("context_export_unavailable", "This native runtime cannot export its complete active context");
  }
  if (!canExit() || !ctx.isIdle() || ctx.hasPendingMessages()) {
    throw new NativeContextError("source_busy", "Finish or interrupt the current turn and pending work before switching");
  }
  const draft = ctx.ui.getEditorText();
  if (typeof draft !== "string" || draft.length !== 0) {
    throw new NativeContextError("source_has_draft", "Submit or save the unsent draft before switching; no draft was changed");
  }
  const header = manager.getHeader();
  const sessionId = manager.getSessionId();
  const sessionFile = manager.getSessionFile();
  if (!header || header.id !== sessionId || typeof sessionFile !== "string" || !path.isAbsolute(sessionFile)) {
    throw new NativeContextError("source_not_persistent", "The original native session must be saved before it can be switched");
  }
  const projection = manager.buildSessionProjection();
  const roles = new Set(["system", "user", "assistant", "toolResult", "bashExecution", "custom", "branchSummary", "compactionSummary"]);
  if (typeof projectForModel !== "function" || !Array.isArray(projection?.messages) ||
      projection.messages.some(message => !roles.has(message?.role))) {
    throw new NativeContextError("context_export_unavailable", "The native model-context projection is not qualified; no context was discarded");
  }
  // The native SDK owns custom-message conversion and context exclusions.
  // Its projection also preserves the active compaction/branch checkpoint.
  const modelMessages = projectForModel(projection.messages);
  const expected = projection.messages.filter(message => !(message.role === "bashExecution" && message.excludeFromContext)).length;
  if (!Array.isArray(modelMessages) || modelMessages.length !== expected) {
    throw new NativeContextError("context_export_unavailable", "The native model-context projection discarded an unexpected record");
  }
  const snapshot = {
    version: 1, format: "pi.session.v3", session_id: sessionId,
    session_path: sessionFile, leaf_id: manager.getLeafId(), cwd: ctx.cwd,
    header, branch: manager.getBranch(), projection, model_messages: modelMessages,
    system_prompt: ctx.getSystemPrompt(),
  };
  const serialized = JSON.stringify(snapshot);
  if (Buffer.byteLength(serialized, "utf8") > 256 * 1024 * 1024) {
    throw new NativeContextError("context_too_large", "Native context exceeds the exchange limit; nothing was truncated or changed");
  }
  // Detach from mutable native entries while the acknowledgement is being written.
  return JSON.parse(serialized);
}

function contextSnapshotId(snapshot: unknown): string {
  return crypto.createHash("sha256").update(JSON.stringify(snapshot)).digest("hex");
}

/** Returns a deferred close callback so a caller can flush its acknowledgement first. */
export function prepareNativeContextCommand(ctx: any, canExit: () => boolean, request: any, capability: string, projectForModel: ModelProjection) {
  const provided = typeof request?.token === "string" ? Buffer.from(request.token) : Buffer.alloc(0);
  const expected = Buffer.from(capability);
  if (expected.length < 16 || provided.length !== expected.length || !crypto.timingSafeEqual(provided, expected)) {
    throw new NativeContextError("context_not_authorized", "The native context capability does not match");
  }
  const snapshot = nativeContextSnapshot(ctx, canExit, projectForModel);
  const snapshotId = contextSnapshotId(snapshot);
  if (request.action === "snapshot") {
    return { body: { ok: true, snapshot_id: snapshotId, snapshot } };
  }
  if (request.action !== "close" || request.snapshot_id !== snapshotId) {
    throw new NativeContextError("source_changed", "The active source context changed; no process was closed");
  }
  return {
    body: { ok: true, closing: true },
    afterFlush: () => {
      // User input or a session change can arrive while the socket is flushing.
      // A controller must still wait for the native process to return to its shell.
      try {
        if (contextSnapshotId(nativeContextSnapshot(ctx, canExit, projectForModel)) === snapshotId) {
          ctx.shutdown();
          return true;
        }
      } catch { /* Preserve the source on any late change or invalidated context. */ }
      return false;
    },
  };
}


const HERDR_ENV = process.env.HERDR_ENV;

/** One private, session-scoped endpoint. It exists only after session_start. */
export async function startNativeContextServer(ctx: any, canExit: () => boolean,
    register: (endpoint: string, capability: string) => Promise<boolean>, projectForModel: ModelProjection) {
  if (process.platform === "win32") throw new NativeContextError("context_export_unavailable", "Native context transport is not qualified on this platform");
  const directory = fs.mkdtempSync("/var/tmp/ohctx-");
  fs.chmodSync(directory, 0o700);
  const endpoint = path.join(directory, "context.sock");
  const capability = crypto.randomUUID();
  const clients = new Set<net.Socket>();
  let closePromise: Promise<void> | undefined;
  let closeRequested = false;
  let disposed = false;
  let socketIdentity: { dev: number; ino: number } | undefined;
  const server = net.createServer(socket => {
    if (closeRequested || clients.size >= 4) { socket.destroy(); return; }
    clients.add(socket);
    socket.once("close", () => clients.delete(socket));
    socket.on("error", () => {});
    socket.setTimeout(5_000, () => socket.destroy());
    let requestBytes = Buffer.alloc(0);
    let handled = false;
    socket.on("data", bytes => {
      if (handled) return;
      requestBytes = Buffer.concat([requestBytes, bytes]);
      if (requestBytes.length > 8_192) { handled = true; socket.destroy(); return; }
      const newline = requestBytes.indexOf(10);
      if (newline < 0) return;
      handled = true;
      try {
        const command = JSON.parse(requestBytes.subarray(0, newline).toString("utf8"));
        const prepared = prepareNativeContextCommand(ctx, () => !disposed && canExit(), command, capability, projectForModel);
        if (prepared.afterFlush) closeRequested = true;
        socket.end(`${JSON.stringify(prepared.body)}\n`, () => {
          if (prepared.afterFlush && !prepared.afterFlush()) closeRequested = false;
        });
      } catch (error) {
        const body = error instanceof NativeContextError
          ? { code: error.code, message: error.message }
          : { code: "context_export_failed", message: "The native context could not be exported; the source was not changed" };
        socket.end(`${JSON.stringify({ ok: false, error: body })}\n`);
      }
    });
  });
  function close(): Promise<void> {
    disposed = true;
    if (closePromise) return closePromise;
    closePromise = new Promise(resolve => {
      server.close(() => {
        try {
          const current = fs.lstatSync(endpoint);
          if (current.isSocket() && socketIdentity && current.dev === socketIdentity.dev && current.ino === socketIdentity.ino) fs.unlinkSync(endpoint);
        } catch { /* The native runtime may already have removed its socket. */ }
        try { fs.rmdirSync(directory); } catch { /* Never recursively remove anything another owner added. */ }
        resolve();
      });
      for (const socket of clients) socket.destroy();
    });
    return closePromise;
  }
  try {
    await new Promise<void>((resolve, reject) => {
      server.once("error", reject);
      server.listen(endpoint, () => { server.removeListener("error", reject); resolve(); });
    });
    server.on("error", () => { void close(); });
    socketIdentity = fs.lstatSync(endpoint);
    fs.chmodSync(endpoint, 0o600);
    server.unref();
    if (!(await register(endpoint, capability))) throw new NativeContextError("context_registration_rejected", "The connected server cannot register native context exchange");
    return { endpoint, close };
  } catch (error) {
    await close();
    throw error;
  }
}

const socketPath = process.env.HERDR_SOCKET_PATH;
const socketEndpoint =
  process.platform === "win32" && socketPath ? `\\\\.\\pipe\\${socketPath}` : socketPath;
const paneId = process.env.HERDR_PANE_ID;
const source = "herdr:pi";

function enabled() {
  return HERDR_ENV === "1" && !!socketPath && !!paneId;
}

function sendRequestAttempt(request: unknown, timeoutMs: number,
    verify?: (response: any) => boolean): Promise<boolean> {
  if (!enabled()) {
    return Promise.resolve(true);
  }

  return new Promise((resolve) => {
    let done = false;
    let timeout: ReturnType<typeof setTimeout> | undefined;
    const finish = (delivered: boolean) => {
      if (done) return;
      done = true;
      if (timeout) {
        clearTimeout(timeout);
      }
      socket.destroy();
      resolve(delivered);
    };

    const socket = net.createConnection(socketEndpoint!);
    socket.on("error", () => finish(false));
    socket.on("connect", () => socket.write(`${JSON.stringify(request)}\n`));
    let responseBytes = Buffer.alloc(0);
    socket.on("data", (bytes) => {
      if (!verify) { finish(true); return; }
      responseBytes = Buffer.concat([responseBytes, bytes]);
      if (responseBytes.length > 32_768) { finish(false); return; }
      const newline = responseBytes.indexOf(10);
      if (newline < 0) return;
      try { finish(verify(JSON.parse(responseBytes.subarray(0, newline).toString("utf8")))); }
      catch { finish(false); }
    });
    socket.on("end", () => finish(false));
    timeout = setTimeout(() => finish(false), timeoutMs);
    timeout.unref?.();
  });
}

async function sendRequest(request: unknown): Promise<void> {
  if (await sendRequestAttempt(request, 500)) {
    return;
  }
  await sendRequestAttempt(request, 1500);
}

type AgentState = "working" | "blocked" | "idle";

type QueuedState = {
  state: AgentState;
  message?: string;
  seq: number;
};

let reportSeq = Date.now() * 1000;
let currentAgentSessionId: string | undefined;
let currentAgentSessionPath: string | undefined;

function nextReportSeq(): number {
  reportSeq += 1;
  return reportSeq;
}

function updateSessionRef(ctx: any): void {
  try {
    const file = ctx?.sessionManager?.getSessionFile?.();
    currentAgentSessionPath =
      typeof file === "string" &&
      (path.posix.isAbsolute(file) || path.win32.isAbsolute(file))
        ? file
        : undefined;
  } catch {
    currentAgentSessionPath = undefined;
  }

  try {
    const id = ctx?.sessionManager?.getSessionId?.();
    currentAgentSessionId = typeof id === "string" && id.length > 0 ? id : undefined;
  } catch {
    currentAgentSessionId = undefined;
  }
}

function withSessionRef(params: Record<string, unknown>): Record<string, unknown> {
  if (currentAgentSessionPath) {
    return { ...params, agent_session_path: currentAgentSessionPath };
  }
  if (currentAgentSessionId) {
    return { ...params, agent_session_id: currentAgentSessionId };
  }
  return params;
}

function currentSessionRef(): Record<string, unknown> | undefined {
  if (currentAgentSessionPath) {
    return { agent_session_path: currentAgentSessionPath };
  }
  if (currentAgentSessionId) {
    return { agent_session_id: currentAgentSessionId };
  }
  return undefined;
}

function reportSession(sessionStartSource?: string): Promise<void> {
  const sessionRef = currentSessionRef();
  if (!sessionRef) {
    return Promise.resolve();
  }

  return sendRequest({
    id: `${source}:session:${Date.now()}:${Math.random().toString(36).slice(2)}`,
    method: "pane.report_agent_session",
    params: {
      pane_id: paneId,
      source,
      agent: "pi",
      seq: nextReportSeq(),
      session_start_source: sessionStartSource,
      ...sessionRef,
    },
  });
}

function sendState(state: AgentState, message?: string, seq = nextReportSeq()): Promise<void> {
  return sendRequest({
    id: `${source}:${Date.now()}:${Math.random().toString(36).slice(2)}`,
    method: "pane.report_agent",
    params: withSessionRef({
      pane_id: paneId,
      source,
      agent: "pi",
      state,
      message,
      seq,
    }),
  });
}

let sendInFlight = false;
let queuedState: QueuedState | undefined;

function queueState(state: AgentState, message?: string): void {
  queuedState = { state, message, seq: nextReportSeq() };
  if (!sendInFlight) {
    void drainStateQueue();
  }
}

async function drainStateQueue(): Promise<void> {
  if (sendInFlight) {
    return;
  }

  sendInFlight = true;
  try {
    while (queuedState) {
      const next = queuedState;
      queuedState = undefined;
      await sendState(next.state, next.message, next.seq);
    }
  } finally {
    sendInFlight = false;
    if (queuedState) {
      void drainStateQueue();
    }
  }
}

export async function registerNativeContext(ctx: any, endpoint: string, token: string,
    send = sendRequestAttempt,
    wait = (ms: number) => new Promise<void>(resolve => setTimeout(resolve, ms)),
    now = () => performance.now()): Promise<boolean> {
  const params = {
    pane_id: paneId, source, agent: "pi", session_id: ctx.sessionManager.getSessionId(),
    session_path: ctx.sessionManager.getSessionFile(), endpoint, token,
    process_id: process.pid, seq: nextReportSeq(), version: 1,
  };
  // Native startup and the server's process observer are asynchronous. Keep
  // ownership strict, but allow the observer's bounded acquisition window.
  const deadline = now() + 8_000;
  while (now() < deadline) {
    let code: string | undefined;
    const accepted = await send({ id: `${source}:context:${params.seq}`, method: "agent.context.register", params }, 1_500,
      response => { code = response?.error?.code; return response?.result?.type === "ok"; });
    if (accepted) return true;
    if (code !== "native_context_not_owned") return false;
    await wait(100);
  }
  return false;
}

export default function (pi) {
  if (!enabled()) {
    return;
  }

  let agentActive = false;
  let blockedCount = 0;
  let blockedMessage: string | undefined;
  let lastState: AgentState | undefined;
  let lastMessage: string | undefined;
  let rootSession = false;
  let contextGeneration = 0;
  let contextService: Awaited<ReturnType<typeof startNativeContextServer>> | undefined;

  function desiredState() {
    if (blockedCount > 0) {
      return { state: "blocked" as const, message: blockedMessage };
    }
    if (agentActive) {
      return { state: "working" as const, message: undefined };
    }
    return { state: "idle" as const, message: undefined };
  }

  function publishState(force = false) {
    const next = desiredState();
    if (!force && next.state === lastState && next.message === lastMessage) {
      return;
    }
    lastState = next.state;
    lastMessage = next.message;
    queueState(next.state, next.message);
  }

  pi.events.on("herdr:blocked", (data) => {
    if (!rootSession) {
      return;
    }
    if (!data?.active) {
      blockedCount = Math.max(0, blockedCount - 1);
      if (blockedCount === 0) {
        blockedMessage = undefined;
      }
      publishState();
      return;
    }

    blockedCount += 1;
    blockedMessage = data.label;
    publishState();
  });

  pi.on("session_start", async (event, ctx) => {
    // TUI only: RPC/JSON/print modes are headless (no PTY herdr can display),
    // and RPC still reports hasUI=true, so mode is the reliable gate.
    if (ctx?.mode !== "tui") {
      return;
    }
    const generation = ++contextGeneration;
    await contextService?.close();
    contextService = undefined;
    rootSession = true;
    updateSessionRef(ctx);
    await reportSession(event?.reason);
    // A reload can replace this extension mid-run without emitting another agent_start.
    agentActive = ctx?.isIdle?.() === false;
    publishState(true);
    // Older SDKs keep ordinary status/session reporting, without claiming a
    // complete-context switch contract they cannot supply.
    if (process.platform !== "win32" && typeof ctx?.sessionManager?.buildSessionProjection === "function"
        && typeof ctx?.ui?.getEditorText === "function" && typeof ctx?.shutdown === "function") {
      try {
        const { convertToLlm } = await import("@earendil-works/pi-coding-agent");
        if (typeof convertToLlm !== "function") return;
        const service = await startNativeContextServer(ctx,
          () => rootSession && generation === contextGeneration && !agentActive && blockedCount === 0,
          (endpoint, token) => registerNativeContext(ctx, endpoint, token), convertToLlm);
        if (generation !== contextGeneration || !rootSession) await service.close();
        else contextService = service;
      } catch { /* An older/disconnected server leaves native context switching unavailable. */ }
    }
  });

  pi.on("session_shutdown", async () => {
    ++contextGeneration;
    rootSession = false;
    const service = contextService;
    contextService = undefined;
    await service?.close();
  });

  pi.on("agent_start", (_event, ctx) => {
    if (!rootSession) {
      return;
    }
    updateSessionRef(ctx);
    void reportSession();
    agentActive = true;
    publishState();
  });

  pi.on("agent_settled", (_event, ctx) => {
    if (!rootSession || ctx?.isIdle?.() !== true) {
      return;
    }

    agentActive = false;
    publishState();
  });
}
