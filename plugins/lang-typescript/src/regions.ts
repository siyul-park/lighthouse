import path from "node:path";
import ts from "typescript";
import { SNIPPETS_DIR } from "./project/paths.ts";
import { byteLineStarts, positionAtByte } from "./positions.ts";
import type {
  Comment,
  EmbeddedFile,
  EmbeddedSource,
  Edge,
  Fragment,
  FunctionSummary,
  Span,
  Symbol as Sym,
  TestCase,
} from "./wire.ts";

/** The extension a fence's language is analysed under, by the first word of its info string (Linguist's names). */
const EXTENSIONS: Record<string, string> = {
  typescript: ".ts",
  ts: ".ts",
  tsx: ".tsx",
  typescriptreact: ".tsx",
  javascript: ".js",
  js: ".js",
  node: ".js",
};

const MODULE_TAIL = "\nexport {};\n";
const UNMAPPED: [number, number] = [-1, -1];

/** The text a fence is analysed as, one entry per byte telling the host bytes it came from. */
export interface RegionText {
  text: string;
  /** For each UTF-8 byte of `text`, the host byte range it stands for, or UNMAPPED. */
  host: [number, number][];
}

/** One fence as a unit of its own: a file beside its host that uses the project from outside. */
export interface Snippet {
  host: EmbeddedFile;
  source: EmbeddedSource;
  /** The project-relative path of the unit's file. */
  rel: string;
  text: RegionText;
}

/** Puts the spans of the fragment of a fence's file where the host file shows them. */
export class Remap {
  private readonly lines: number[];
  private readonly hostLines: number[];

  constructor(
    private readonly snippet: Snippet,
    private readonly module: string,
  ) {
    this.lines = byteLineStarts(Buffer.from(snippet.text.text));
    this.hostLines = byteLineStarts(Buffer.from(snippet.host.text));
  }

  /**
   * The fragment as the host shows it, with the host path as its own. What
   * the host does not show (the module tail) is dropped with what is about it.
   */
  fragment(source: Fragment): Fragment {
    const dropped = new Set<string>();
    const symbols: Sym[] = [];
    for (const symbol of source.symbols ?? []) {
      const span = this.span(symbol.span);
      if (span === undefined) {
        dropped.add(symbol.id);
        continue;
      }
      const extent = symbol.extent == null ? undefined : this.span(symbol.extent);
      symbols.push({ ...symbol, span, file: this.snippet.host.path, extent: extent ?? undefined });
    }
    for (const symbol of symbols) if (symbol.owner != null && dropped.has(symbol.owner)) symbol.owner = undefined;
    return {
      file: { path: this.snippet.host.path },
      modules: source.modules ?? [],
      symbols,
      edges: this.edges(source.edges ?? [], dropped),
      functions: (source.functions ?? []).filter((f: FunctionSummary) => !dropped.has(f.symbol)),
      tests: (source.tests ?? []).filter((t: TestCase) => !dropped.has(t.symbol)),
      comments: this.comments(source.comments ?? [], dropped),
    };
  }

  private edges(all: readonly Edge[], dropped: ReadonlySet<string>): Edge[] {
    const shown: Edge[] = [];
    for (const edge of all) {
      if (dropped.has(edge.to)) continue;
      const next: Edge = { ...edge };
      if ("symbol" in edge.from && dropped.has(edge.from.symbol)) next.from = { module: this.module };
      if (edge.site != null) {
        const site = this.span(edge.site);
        next.site = site;
      }
      shown.push(next);
    }
    return shown;
  }

  private comments(all: readonly Comment[], dropped: ReadonlySet<string>): Comment[] {
    const shown: Comment[] = [];
    for (const comment of all) {
      const span = this.span(comment.span);
      if (span === undefined) continue;
      const attached =
        comment.attached_to != null && dropped.has(comment.attached_to) ? undefined : comment.attached_to;
      shown.push({ ...comment, span, attached_to: attached });
    }
    return shown;
  }

  /** The host span of a span of the fence's file; undefined where the host shows nothing of it. */
  private span(span: Span): Span | undefined {
    const from = this.offset(span.start);
    const to = this.offset(span.end);
    if (from === undefined || to === undefined) return undefined;
    const at = this.hostRange(from, to);
    if (at === undefined) return undefined;
    return { start: positionAtByte(this.hostLines, at[0]), end: positionAtByte(this.hostLines, at[1]) };
  }

  private offset(position: { line: number; col: number }): number | undefined {
    const start = this.lines[position.line - 1];
    return start === undefined || position.col < 1 ? undefined : start + position.col - 1;
  }

  /** The host bytes of the bytes `start..end`: from where the first starts to where the last the host shows ends. */
  private hostRange(start: number, end: number): [number, number] | undefined {
    const host = this.snippet.text.host;
    const first = host[start];
    if (first === undefined || first[0] === -1) return undefined;
    for (let at = Math.min(Math.max(end, start + 1), host.length) - 1; at >= start; at--) {
      const entry = host[at];
      if (entry !== undefined && entry[0] !== -1) return [first[0], Math.max(entry[1], first[0])];
    }
    return undefined;
  }
}

/** The units of the fences of the host files of a request, in the order of the fences. */
export function planSnippets(hosts: readonly EmbeddedFile[]): Snippet[] {
  const snippets: Snippet[] = [];
  for (const host of hosts) {
    const sources = [...host.sources].sort((a, b) => a.source_range[0] - b.source_range[0]);
    sources.forEach((source, index) => {
      const dir = path.posix.dirname(host.path);
      const name = `${path.posix.basename(host.path)}.${index + 1}`;
      const rel = path.posix.join(dir, SNIPPETS_DIR, name, `snippet${extensionOf(source)}`);
      snippets.push({ host, source, rel, text: wrap(regionText(source)) });
    });
  }
  return snippets;
}

function extensionOf(source: EmbeddedSource): string {
  const info = source.context?.["info"];
  const word =
    typeof info === "string"
      ? info
          .trim()
          .split(/[\s,{]/)[0]
          ?.toLowerCase()
      : undefined;
  return (word === undefined ? undefined : EXTENSIONS[word]) ?? ".ts";
}

/** A fence is a module of its own: one that neither imports nor exports is made one, so its names stay its own. */
function wrap(body: RegionText): RegionText {
  const probe = ts.createSourceFile("snippet.tsx", body.text, ts.ScriptTarget.ESNext, false);
  if (ts.isExternalModule(probe)) return body;
  return around(body, "", MODULE_TAIL);
}

/** The fence as it is written, each byte at its place in the host. */
function regionText(source: EmbeddedSource): RegionText {
  const bytes = Buffer.byteLength(source.text);
  const [from, to] = source.source_range;
  const host: [number, number][] = Array.from({ length: bytes }, () => [from, to]);
  if (source.text_map == null && to - from === bytes) {
    for (let i = 0; i < bytes; i++) host[i] = [from + i, from + i + 1];
  }
  for (const segment of source.text_map ?? []) {
    const [textFrom, textTo] = segment.text_range;
    const [hostFrom, hostTo] = segment.source_range;
    const linear = textTo - textFrom === hostTo - hostFrom;
    for (let i = textFrom; i < textTo && i < host.length; i++) {
      host[i] = linear ? [hostFrom + i - textFrom, hostFrom + i - textFrom + 1] : [hostFrom, hostTo];
    }
  }
  return { text: source.text, host };
}

function around(body: RegionText, head: string, tail: string): RegionText {
  const hidden = (text: string): [number, number][] => Array.from({ length: Buffer.byteLength(text) }, () => UNMAPPED);
  return { text: head + body.text + tail, host: [...hidden(head), ...body.host, ...hidden(tail)] };
}
