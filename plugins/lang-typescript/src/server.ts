import type { Readable, Writable } from "node:stream";
import { StreamMessageReader, StreamMessageWriter } from "vscode-jsonrpc/node";
import type { IndexParams, IndexResult, InitializeParams, InitializeResult } from "./wire.ts";

export const PROTOCOL_VERSION = "0.4";

const PARSE_ERROR = -32700;

const METHOD_NOT_FOUND = -32601;
const INVALID_PARAMS = -32602;
const INTERNAL_ERROR = -32603;
const REQUEST_FAILED = -32803;
const NOT_INITIALIZED = -32002;
/** The methods of a language provider. */
export interface Handler {
  initialize(params: InitializeParams): InitializeResult;
  index(params: IndexParams): IndexResult;
}

interface Incoming {
  id?: number | string | null;
  method?: string;
  params?: unknown;
}

interface Failure {
  code: number;
  message: string;
}

/** What the loop knows of the conversation. */
interface Session {
  initialized: boolean;
  shutdown: boolean;
}

type Outcome = { result: unknown } | Failure;

/** The requests of one conversation, answered in the order they arrive. */
class Loop {
  private queue: Promise<void> = Promise.resolve();
  private done = false;
  private readonly session: Session = { initialized: false, shutdown: false };

  constructor(
    private readonly handler: Handler,
    private readonly writer: StreamMessageWriter,
    private readonly resolve: (code: number) => void,
  ) {}

  attach(reader: StreamMessageReader): void {
    reader.onError((error) =>
      this.enqueue(async () => {
        await reply(this.writer, null, { code: PARSE_ERROR, message: error.message });
        this.finish(1);
      }),
    );
    reader.onClose(() => this.enqueue(() => this.finish(this.exitCode())));
    reader.listen((message) => this.enqueue(() => this.handle(message as Incoming)));
  }

  private enqueue(task: () => void | Promise<void>): void {
    this.queue = this.queue.then(task);
  }

  private exitCode(): number {
    return this.session.shutdown ? 0 : 1;
  }

  private finish(code: number): void {
    if (this.done) return;
    this.done = true;
    this.resolve(code);
  }

  private async handle(incoming: Incoming): Promise<void> {
    if (this.done) return;
    if (incoming.method === "exit") {
      this.finish(this.exitCode());
      return;
    }
    if (incoming.method === undefined) return;
    const outcome = dispatch(this.handler, incoming, this.session);
    if (incoming.id === undefined || incoming.id === null) return;
    await reply(this.writer, incoming.id, outcome);
  }
}

/**
 * Serves requests from `input` on `output` until the host sends `exit` or
 * closes the input, and returns the exit code: 0 after `shutdown` and `exit`
 * (or an input that closes after `shutdown`), 1 otherwise.
 */
export function serve(input: Readable, output: Writable, handler: Handler): Promise<number> {
  return new Promise((resolve) => {
    new Loop(handler, new StreamMessageWriter(output), resolve).attach(new StreamMessageReader(input));
  });
}

async function reply(writer: StreamMessageWriter, id: number | string | null, outcome: Outcome): Promise<void> {
  const body =
    "result" in outcome
      ? { jsonrpc: "2.0", id, result: outcome.result }
      : { jsonrpc: "2.0", id, error: { code: outcome.code, message: outcome.message } };
  await writer.write(body as never);
}

function dispatch(handler: Handler, message: Incoming, session: Session): Outcome {
  try {
    return route(handler, message, session);
  } catch (error) {
    const reason = error instanceof Error ? error.message : String(error);
    return { code: INTERNAL_ERROR, message: `panic: ${reason}` };
  }
}

function route(handler: Handler, message: Incoming, session: Session): Outcome {
  switch (message.method) {
    case "initialize": {
      const params = decode<InitializeParams>(message.params, ["protocolVersion", "clientInfo", "workspaceFolders"]);
      if ("code" in params) return params;
      if (params.protocolVersion !== PROTOCOL_VERSION) {
        return {
          code: REQUEST_FAILED,
          message: `host speaks protocol ${params.protocolVersion}, this plugin speaks ${PROTOCOL_VERSION}`,
        };
      }
      const result = handler.initialize(params);
      session.initialized = true;
      return { result };
    }
    default:
      break;
  }
  if (!session.initialized) {
    return { code: NOT_INITIALIZED, message: `\`${message.method}\` before \`initialize\`` };
  }
  switch (message.method) {
    case "lighthouse/index": {
      const params = decode<IndexParams>(message.params, ["project", "language", "inputs"]);
      if ("code" in params) return params;
      return { result: handler.index(params) };
    }
    case "shutdown":
      session.shutdown = true;
      return { result: null };
    default:
      return { code: METHOD_NOT_FOUND, message: `unknown method ${message.method}` };
  }
}

/** Refuses params that lack a required member, as the Rust server does for the fields its types require. */
function decode<T>(params: unknown, required: readonly string[]): T | Failure {
  if (typeof params !== "object" || params === null || Array.isArray(params)) {
    return { code: INVALID_PARAMS, message: "params must be an object" };
  }
  for (const name of required) {
    if (!(name in params)) return { code: INVALID_PARAMS, message: `missing field \`${name}\`` };
  }
  return params as T;
}
