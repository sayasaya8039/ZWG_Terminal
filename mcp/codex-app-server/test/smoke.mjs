import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { once } from "node:events";
import { createInterface } from "node:readline";

const child = spawn(process.execPath, ["server.mjs"], {
  cwd: new URL("..", import.meta.url),
  env: process.env,
  stdio: ["pipe", "pipe", "pipe"],
});
const output = createInterface({ input: child.stdout, crlfDelay: Infinity });
let nextId = 1;
const pending = new Map();

output.on("line", (line) => {
  if (!line.trim()) {
    return;
  }
  const message = JSON.parse(line);
  const waiter = pending.get(message.id);
  if (!waiter) {
    return;
  }
  pending.delete(message.id);
  if (message.error) {
    waiter.reject(new Error(JSON.stringify(message.error)));
  } else {
    waiter.resolve(message.result);
  }
});

child.stderr.on("data", (chunk) => {
  process.stderr.write(chunk);
});

function request(method, params = {}) {
  const id = nextId++;
  child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Timed out waiting for ${method}`)), 30000);
    pending.set(id, {
      resolve(value) {
        clearTimeout(timeout);
        resolve(value);
      },
      reject(error) {
        clearTimeout(timeout);
        reject(error);
      },
    });
  });
}

try {
  await request("initialize", {
    protocolVersion: "2024-11-05",
    clientInfo: { name: "smoke", version: "0.0.0" },
    capabilities: {},
  });

  const result = await request("tools/call", {
    name: "codex_auth_status",
    arguments: {},
  });
  assert.equal(result.content[0].type, "text");
  const status = JSON.parse(result.content[0].text);
  assert.notEqual(status.authMethod, undefined);
  assert.equal(status.authToken, null);
} finally {
  child.stdin.end();
  const [code] = await once(child, "exit");
  assert.equal(code, 0);
}
