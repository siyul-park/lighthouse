import assert from "node:assert/strict";
import { PassThrough } from "node:stream";
import { describe, it } from "node:test";
import { Provider } from "../src/provider.ts";
import { serve } from "../src/server.ts";

const INITIALIZE = {
  protocolVersion: "0.4",
  clientInfo: { name: "test", version: "0" },
  workspaceFolders: [{ uri: "file:///workspace", name: "workspace" }],
};

interface Reply {
  id: number | null;
  result?: unknown;
  error?: { code: number; message: string };
}

function frame(message: object): string {
  const body = JSON.stringify({ jsonrpc: "2.0", ...message });
  return `Content-Length: ${Buffer.byteLength(body)}\r\n\r\n${body}`;
}

function parse(chunks: Buffer[]): Reply[] {
  const text = Buffer.concat(chunks).toString();
  return [...text.matchAll(/Content-Length: \d+\r\n\r\n(\{.*?\})(?=Content-Length|$)/gs)].map(
    (match) => JSON.parse(match[1]!) as Reply,
  );
}

/**
 * Feeds the text to a served provider and returns its exit code and the replies it wrote.
 * The input stays open until `expected` replies were written, as a host's does while it waits.
 */
async function converse(input: string, expected: number): Promise<{ code: number; replies: Reply[] }> {
  const stdin = new PassThrough();
  const stdout = new PassThrough();
  const chunks: Buffer[] = [];
  stdout.on("data", (chunk: Buffer) => chunks.push(chunk));
  const done = serve(stdin, stdout, new Provider("lang-typescript", "0"));
  stdin.write(input);
  while (parse(chunks).length < expected) await new Promise((resolve) => setImmediate(resolve));
  stdin.end();
  const code = await done;
  return { code, replies: parse(chunks) };
}

describe("the protocol loop", () => {
  it("answers the lifecycle and exits cleanly after shutdown and exit", async () => {
    const { code, replies } = await converse(
      frame({ id: 1, method: "initialize", params: INITIALIZE }) +
        frame({ id: 2, method: "shutdown" }) +
        frame({ method: "exit" }),
      2,
    );

    assert.equal(code, 0);
    assert.equal(replies.length, 2);
    assert.equal(replies[1]!.result, null);
  });

  it("declares the language, its aliases and its honest capabilities", async () => {
    const { replies } = await converse(frame({ id: 1, method: "initialize", params: INITIALIZE }), 1);

    const result = replies[0]!.result as {
      serverInfo: { name: string };
      capabilities: { languages: { id: string; aliases: string[]; capabilities: string[] }[] };
    };
    const [language] = result.capabilities.languages;
    assert.equal(result.serverInfo.name, "lang-typescript");
    assert.equal(language?.id, "typescript");
    assert.deepEqual(language?.aliases, ["typescript", "ts", "tsx", "typescriptreact", "javascript", "js", "node"]);
    assert.deepEqual(language?.capabilities, ["semantic-edges", "extent", "reference-sites", "overlays"]);
  });

  it("refuses a request before initialize, another protocol version and unknown methods", async () => {
    const { replies } = await converse(
      frame({ id: 1, method: "lighthouse/index", params: {} }) +
        frame({ id: 2, method: "initialize", params: { ...INITIALIZE, protocolVersion: "0.0" } }) +
        frame({ id: 3, method: "initialize", params: INITIALIZE }) +
        frame({ id: 4, method: "lighthouse/nope", params: {} }) +
        frame({ id: 5, method: "lighthouse/index", params: {} }),
      5,
    );

    assert.deepEqual(
      replies.map((reply) => reply.error?.code),
      [-32002, -32803, undefined, -32601, -32602],
    );
  });

  it("answers garbage with a parse error and ends with a failure", async () => {
    const { code, replies } = await converse("Content-Length: 2\r\n\r\n{]", 1);

    assert.equal(code, 1);
    assert.equal(replies[0]?.error?.code, -32700);
  });

  it("ends with a failure when the input closes before shutdown", async () => {
    const { code } = await converse(frame({ id: 1, method: "initialize", params: INITIALIZE }), 1);

    assert.equal(code, 1);
  });
});
