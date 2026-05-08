#!/usr/bin/env node

import { spawn } from "node:child_process";
import { EventEmitter } from "node:events";
import { existsSync } from "node:fs";
import { createInterface } from "node:readline";
import path from "node:path";
import process from "node:process";
import { fileURLToPath } from "node:url";

const SERVER_NAME = "codex-app-server";
const SERVER_VERSION = "0.1.1";
const DEFAULT_PROTOCOL_VERSION = "2024-11-05";
const DEFAULT_TURN_TIMEOUT_MS = 10 * 60 * 1000;
const BRIDGE_DIR = path.dirname(fileURLToPath(import.meta.url));
const DEFAULT_CODEX_CWD =
  process.env.CODEX_APP_SERVER_DEFAULT_CWD ?? path.resolve(BRIDGE_DIR, "..", "..");

const TOOLS = [
  {
    name: "codex_chat",
    description:
      "Start a new codex app-server thread, send one prompt, and wait for the turn to finish.",
    inputSchema: {
      type: "object",
      required: ["prompt"],
      properties: {
        prompt: { type: "string", minLength: 1 },
        cwd: {
          type: "string",
          description: "Working directory for the Codex thread. Defaults to the MCP process cwd.",
        },
        model: { type: "string" },
        effort: {
          type: "string",
          enum: ["low", "medium", "high", "xhigh"],
          description: "Optional Codex reasoning effort override.",
        },
        approvalPolicy: {
          type: "string",
          enum: ["never", "untrusted", "on-failure", "on-request"],
          default: "never",
        },
        sandbox: {
          type: "string",
          enum: ["read-only", "workspace-write", "danger-full-access"],
          default: "read-only",
        },
        timeoutMs: {
          type: "integer",
          minimum: 1000,
          default: DEFAULT_TURN_TIMEOUT_MS,
        },
        ephemeral: {
          type: "boolean",
          default: false,
          description: "When true, asks Codex not to materialize the thread on disk.",
        },
        includeItems: {
          type: "boolean",
          default: false,
          description: "Include completed turn items in the returned JSON.",
        },
        baseInstructions: { type: "string" },
        developerInstructions: { type: "string" },
      },
      additionalProperties: false,
    },
  },
  {
    name: "codex_continue",
    description:
      "Continue an existing codex app-server thread with a new prompt and wait for completion.",
    inputSchema: {
      type: "object",
      required: ["threadId", "prompt"],
      properties: {
        threadId: { type: "string", minLength: 1 },
        prompt: { type: "string", minLength: 1 },
        cwd: { type: "string" },
        model: { type: "string" },
        effort: {
          type: "string",
          enum: ["low", "medium", "high", "xhigh"],
        },
        approvalPolicy: {
          type: "string",
          enum: ["never", "untrusted", "on-failure", "on-request"],
        },
        sandbox: {
          type: "string",
          enum: ["read-only", "workspace-write", "danger-full-access"],
        },
        timeoutMs: {
          type: "integer",
          minimum: 1000,
          default: DEFAULT_TURN_TIMEOUT_MS,
        },
        includeItems: { type: "boolean", default: false },
      },
      additionalProperties: false,
    },
  },
  {
    name: "codex_thread_read",
    description: "Read a codex app-server thread by id.",
    inputSchema: {
      type: "object",
      required: ["threadId"],
      properties: {
        threadId: { type: "string", minLength: 1 },
        includeTurns: { type: "boolean", default: true },
      },
      additionalProperties: false,
    },
  },
  {
    name: "codex_auth_status",
    description: "Read Codex authentication status without returning auth tokens.",
    inputSchema: {
      type: "object",
      properties: {},
      additionalProperties: false,
    },
  },
  {
    name: "codex_model_list",
    description: "List models reported by codex app-server.",
    inputSchema: {
      type: "object",
      properties: {
        limit: { type: "integer", minimum: 1, maximum: 200 },
        cursor: { type: "string" },
        includeHidden: { type: "boolean", default: false },
      },
      additionalProperties: false,
    },
  },
];

class CodexAppServerClient {
  constructor() {
    this.child = null;
    this.nextId = 1;
    this.pending = new Map();
    this.initialized = false;
    this.events = new EventEmitter();
    this.turns = new Map();
  }

  async ensureStarted() {
    if (this.initialized) {
      return;
    }

    if (!this.child) {
      const { command, args } = getCodexAppServerCommand();
      this.child = spawn(command, args, {
        cwd: process.cwd(),
        env: process.env,
        stdio: ["pipe", "pipe", "pipe"],
        shell: shouldUseShell(command),
      });

      this.child.once("error", (error) => {
        this.rejectAllPending(error);
        this.child = null;
        this.initialized = false;
      });

      this.child.once("exit", (code, signal) => {
        const error = new Error(`codex app-server exited: code=${code} signal=${signal}`);
        this.rejectAllPending(error);
        this.child = null;
        this.initialized = false;
      });

      this.child.stderr.on("data", (chunk) => {
        process.stderr.write(`[codex-app-server] ${chunk}`);
      });

      const output = createInterface({ input: this.child.stdout, crlfDelay: Infinity });
      output.on("line", (line) => {
        this.handleLine(line);
      });
    }

    await this.request("initialize", {
      clientInfo: {
        name: "zwg-codex-app-server-mcp",
        title: "ZWG Codex App Server MCP",
        version: SERVER_VERSION,
      },
      capabilities: {
        experimentalApi: true,
        optOutNotificationMethods: [],
      },
    });
    this.initialized = true;
  }

  handleLine(line) {
    const trimmed = line.trim();
    if (!trimmed) {
      return;
    }

    let message;
    try {
      message = JSON.parse(trimmed);
    } catch (error) {
      process.stderr.write(`[codex-app-server] invalid JSON: ${error.message}\n`);
      return;
    }

    if (Object.prototype.hasOwnProperty.call(message, "id") && !message.method) {
      const pending = this.pending.get(message.id);
      if (!pending) {
        return;
      }
      this.pending.delete(message.id);
      if (message.error) {
        pending.reject(new Error(formatJson(message.error)));
      } else {
        pending.resolve(message.result);
      }
      return;
    }

    if (message.method && Object.prototype.hasOwnProperty.call(message, "id")) {
      this.respondToServerRequest(message);
      return;
    }

    if (message.method) {
      this.handleNotification(message);
    }
  }

  handleNotification(message) {
    const params = message.params ?? {};
    if (message.method === "item/agentMessage/delta") {
      const key = turnKey(params.threadId, params.turnId);
      const state = this.turns.get(key) ?? { deltas: "", completed: null };
      state.deltas += params.delta ?? "";
      this.turns.set(key, state);
      return;
    }

    if (message.method === "turn/completed") {
      const turn = params.turn;
      const key = turnKey(params.threadId, turn?.id);
      const state = this.turns.get(key) ?? { deltas: "", completed: null };
      state.completed = params;
      this.turns.set(key, state);
      this.events.emit(key, state);
      return;
    }

    if (message.method === "error" && params.threadId && params.turnId) {
      const key = turnKey(params.threadId, params.turnId);
      const state = this.turns.get(key) ?? { deltas: "", completed: null };
      state.completed = {
        threadId: params.threadId,
        turn: {
          id: params.turnId,
          items: [],
          itemsView: "full",
          status: "failed",
          error: params.error ?? null,
          startedAt: null,
          completedAt: null,
          durationMs: null,
        },
      };
      this.turns.set(key, state);
      this.events.emit(key, state);
    }
  }

  respondToServerRequest(message) {
    const method = message.method;
    let result;

    switch (method) {
      case "item/commandExecution/requestApproval":
        result = { decision: "decline" };
        break;
      case "item/fileChange/requestApproval":
        result = { decision: "decline" };
        break;
      case "item/permissions/requestApproval":
        result = { permissions: {}, scope: "turn", strictAutoReview: true };
        break;
      case "item/tool/requestUserInput":
        result = { answers: {} };
        break;
      case "mcpServer/elicitation/request":
        result = { action: "decline", content: null, _meta: null };
        break;
      case "item/tool/call":
        result = {
          success: false,
          contentItems: [
            {
              type: "inputText",
              text: "Denied by codex-app-server MCP bridge.",
            },
          ],
        };
        break;
      case "applyPatchApproval":
      case "execCommandApproval":
        result = { decision: "denied" };
        break;
      case "account/chatgptAuthTokens/refresh":
        this.writeToApp({
          id: message.id,
          error: {
            code: -32001,
            message: "Token refresh must be handled by the Codex CLI session, not this MCP bridge.",
          },
        });
        return;
      default:
        this.writeToApp({
          id: message.id,
          error: {
            code: -32601,
            message: `Unsupported app-server request: ${method}`,
          },
        });
        return;
    }

    this.writeToApp({ id: message.id, result });
  }

  async request(method, params) {
    if (!this.child?.stdin?.writable) {
      throw new Error("codex app-server is not running");
    }

    const id = this.nextId++;
    const promise = new Promise((resolve, reject) => {
      this.pending.set(id, { resolve, reject });
    });
    this.writeToApp({ id, method, params });
    return promise;
  }

  writeToApp(message) {
    this.child.stdin.write(`${JSON.stringify(message)}\n`);
  }

  async startThread(args) {
    await this.ensureStarted();
    const params = {
      cwd: args.cwd ?? DEFAULT_CODEX_CWD,
      approvalPolicy: args.approvalPolicy ?? "never",
      sandbox: args.sandbox ?? "read-only",
      ephemeral: args.ephemeral ?? false,
      experimentalRawEvents: false,
      persistExtendedHistory: false,
    };
    copyIfPresent(params, args, ["model", "baseInstructions", "developerInstructions"]);
    return this.request("thread/start", params);
  }

  async startTurn(args) {
    await this.ensureStarted();
    const params = {
      threadId: args.threadId,
      input: [{ type: "text", text: args.prompt, text_elements: [] }],
    };
    copyIfPresent(params, args, ["cwd", "approvalPolicy", "model", "effort"]);
    if (args.sandbox) {
      params.sandboxPolicy = toSandboxPolicy(args.sandbox, args.cwd ?? DEFAULT_CODEX_CWD);
    }

    const response = await this.request("turn/start", params);
    const turnId = response?.turn?.id;
    if (!turnId) {
      throw new Error(`turn/start response did not include turn.id: ${formatJson(response)}`);
    }

    const state = await this.waitForTurn(args.threadId, turnId, args.timeoutMs);
    return summarizeTurn({
      threadId: args.threadId,
      turnId,
      turn: state.completed.turn,
      deltas: state.deltas,
      includeItems: args.includeItems === true,
    });
  }

  async waitForTurn(threadId, turnId, timeoutMs = DEFAULT_TURN_TIMEOUT_MS) {
    const key = turnKey(threadId, turnId);
    const existing = this.turns.get(key);
    if (existing?.completed) {
      return existing;
    }

    return new Promise((resolve, reject) => {
      const timer = setTimeout(() => {
        this.events.off(key, onComplete);
        reject(new Error(`Timed out waiting for Codex turn ${turnId}`));
      }, timeoutMs);

      const onComplete = (state) => {
        clearTimeout(timer);
        resolve(state);
      };

      this.events.once(key, onComplete);
    });
  }

  rejectAllPending(error) {
    for (const pending of this.pending.values()) {
      pending.reject(error);
    }
    this.pending.clear();
  }

  close() {
    this.rejectAllPending(new Error("codex-app-server MCP bridge is shutting down"));

    if (this.child && !this.child.killed) {
      this.child.kill();
    }
    this.child = null;
    this.initialized = false;
  }
}

class McpServer {
  constructor() {
    this.codex = new CodexAppServerClient();
  }

  async handle(message) {
    if (!message || typeof message !== "object") {
      return;
    }

    if (!Object.prototype.hasOwnProperty.call(message, "id")) {
      return;
    }

    try {
      switch (message.method) {
        case "initialize":
          return this.result(message.id, {
            protocolVersion: message.params?.protocolVersion ?? DEFAULT_PROTOCOL_VERSION,
            capabilities: {
              tools: {},
            },
            serverInfo: {
              name: SERVER_NAME,
              version: SERVER_VERSION,
            },
          });
        case "ping":
          return this.result(message.id, {});
        case "tools/list":
          return this.result(message.id, { tools: TOOLS });
        case "tools/call":
          return this.result(message.id, await this.callTool(message.params ?? {}));
        case "resources/list":
          return this.result(message.id, { resources: [] });
        case "prompts/list":
          return this.result(message.id, { prompts: [] });
        case "logging/setLevel":
          return this.result(message.id, {});
        default:
          return this.error(message.id, -32601, `Method not found: ${message.method}`);
      }
    } catch (error) {
      return this.error(message.id, -32603, error?.stack ?? String(error));
    }
  }

  async callTool(params) {
    const name = params.name;
    const args = params.arguments ?? {};

    try {
      switch (name) {
        case "codex_chat": {
          requireString(args.prompt, "prompt");
          const threadResponse = await this.codex.startThread(args);
          const threadId = threadResponse?.thread?.id;
          if (!threadId) {
            throw new Error(
              `thread/start response did not include thread.id: ${formatJson(threadResponse)}`,
            );
          }
          const summary = await this.codex.startTurn({ ...args, threadId });
          summary.thread = {
            id: threadId,
            model: threadResponse.model,
            cwd: threadResponse.cwd,
          };
          return textResult(summary);
        }
        case "codex_continue": {
          requireString(args.threadId, "threadId");
          requireString(args.prompt, "prompt");
          const summary = await this.codex.startTurn(args);
          return textResult(summary);
        }
        case "codex_thread_read": {
          requireString(args.threadId, "threadId");
          await this.codex.ensureStarted();
          const response = await this.codex.request("thread/read", {
            threadId: args.threadId,
            includeTurns: args.includeTurns !== false,
          });
          return textResult(response);
        }
        case "codex_auth_status": {
          await this.codex.ensureStarted();
          const response = await this.codex.request("getAuthStatus", {
            includeToken: false,
            refreshToken: false,
          });
          return textResult({ ...response, authToken: response?.authToken ? "<redacted>" : null });
        }
        case "codex_model_list": {
          await this.codex.ensureStarted();
          const response = await this.codex.request("model/list", {
            cursor: args.cursor ?? null,
            limit: args.limit ?? null,
            includeHidden: args.includeHidden === true,
          });
          return textResult(response);
        }
        default:
          return {
            isError: true,
            content: [{ type: "text", text: `Unknown tool: ${name}` }],
          };
      }
    } catch (error) {
      return {
        isError: true,
        content: [{ type: "text", text: error?.stack ?? String(error) }],
      };
    }
  }

  result(id, result) {
    sendToMcp({ jsonrpc: "2.0", id, result });
  }

  error(id, code, message) {
    sendToMcp({
      jsonrpc: "2.0",
      id,
      error: { code, message },
    });
  }
}

function getCodexAppServerCommand() {
  const args = parseArgs(process.env.CODEX_APP_SERVER_ARGS) ?? [
    "app-server",
    "--listen",
    "stdio://",
  ];

  if (process.env.CODEX_APP_SERVER_COMMAND) {
    return { command: process.env.CODEX_APP_SERVER_COMMAND, args };
  }

  const codexJs = findNpmCodexEntrypoint();
  if (codexJs) {
    return { command: process.execPath, args: [codexJs, ...args] };
  }

  return { command: "codex", args };
}

function findNpmCodexEntrypoint() {
  const appData = process.env.APPDATA;
  if (!appData) {
    return null;
  }

  const candidate = path.join(appData, "npm", "node_modules", "@openai", "codex", "bin", "codex.js");
  return existsSync(candidate) ? candidate : null;
}

function shouldUseShell(command) {
  if (process.platform !== "win32") {
    return false;
  }
  const lower = command.toLowerCase();
  return lower === "codex" || lower.endsWith(".cmd") || lower.endsWith(".bat") || lower.endsWith(".ps1");
}

function parseArgs(value) {
  if (!value) {
    return null;
  }
  const trimmed = value.trim();
  if (!trimmed) {
    return null;
  }
  if (trimmed.startsWith("[")) {
    const parsed = JSON.parse(trimmed);
    if (!Array.isArray(parsed) || !parsed.every((item) => typeof item === "string")) {
      throw new Error("CODEX_APP_SERVER_ARGS JSON must be an array of strings");
    }
    return parsed;
  }
  return trimmed.match(/(?:[^\s"]+|"[^"]*")+/g)?.map((part) => part.replace(/^"|"$/g, "")) ?? [];
}

function toSandboxPolicy(sandbox, cwd) {
  switch (sandbox) {
    case "danger-full-access":
      return { type: "dangerFullAccess" };
    case "workspace-write":
      return {
        type: "workspaceWrite",
        writableRoots: [cwd],
        networkAccess: true,
        excludeTmpdirEnvVar: false,
        excludeSlashTmp: false,
      };
    case "read-only":
    default:
      return { type: "readOnly", networkAccess: true };
  }
}

function summarizeTurn({ threadId, turnId, turn, deltas, includeItems }) {
  const agentMessages =
    turn?.items
      ?.filter((item) => item.type === "agentMessage")
      .map((item) => item.text)
      .filter(Boolean) ?? [];
  const answer = agentMessages.length > 0 ? agentMessages.join("\n\n") : deltas;
  const summary = {
    threadId,
    turnId,
    status: turn?.status ?? "unknown",
    answer,
    error: turn?.error ?? null,
    startedAt: turn?.startedAt ?? null,
    completedAt: turn?.completedAt ?? null,
    durationMs: turn?.durationMs ?? null,
  };

  if (includeItems) {
    summary.items = turn?.items ?? [];
  }

  return summary;
}

function copyIfPresent(target, source, keys) {
  for (const key of keys) {
    if (source[key] !== undefined && source[key] !== null && source[key] !== "") {
      target[key] = source[key];
    }
  }
}

function requireString(value, name) {
  if (typeof value !== "string" || value.trim() === "") {
    throw new Error(`${name} must be a non-empty string`);
  }
}

function textResult(value) {
  return {
    content: [{ type: "text", text: formatJson(value) }],
  };
}

function formatJson(value) {
  return JSON.stringify(value, null, 2);
}

function turnKey(threadId, turnId) {
  return `${threadId ?? ""}:${turnId ?? ""}`;
}

function sendToMcp(message) {
  process.stdout.write(`${JSON.stringify(message)}\n`);
}

const server = new McpServer();
const input = createInterface({ input: process.stdin, crlfDelay: Infinity });
let shuttingDown = false;

input.on("line", async (line) => {
  const trimmed = line.trim();
  if (!trimmed) {
    return;
  }

  try {
    await server.handle(JSON.parse(trimmed));
  } catch (error) {
    process.stderr.write(`[${SERVER_NAME}] ${error?.stack ?? String(error)}\n`);
  }
});

function shutdown() {
  if (shuttingDown) {
    return;
  }
  shuttingDown = true;
  server.codex.close();
  process.exit(0);
}

input.on("close", shutdown);
process.stdin.on("end", shutdown);
process.stdin.on("close", shutdown);
process.on("SIGINT", shutdown);
process.on("SIGTERM", shutdown);
