import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { after, describe, it } from "node:test";
import { Provider } from "../src/provider.ts";
import type { EmbeddedFile, Fragment, IndexResult } from "../src/wire.ts";

const GREET = "src/index::greet#function";
const MANIFEST = '{ "name": "demo", "private": false }\n';
const LIBRARY = "export function greet(name: string): string {\n  return name;\n}\n";
const TSCONFIG =
  '{ "compilerOptions": { "strict": true, "module": "ESNext", "moduleResolution": "Bundler" }, "include": ["src"] }\n';

const roots: string[] = [];

after(() => {
  for (const root of roots) fs.rmSync(root, { recursive: true, force: true });
});

function project(): string {
  const root = fs.realpathSync(fs.mkdtempSync(path.join(os.tmpdir(), "lang-typescript-")));
  roots.push(root);
  fs.mkdirSync(path.join(root, "src"));
  fs.writeFileSync(path.join(root, "package.json"), MANIFEST);
  fs.writeFileSync(path.join(root, "tsconfig.json"), TSCONFIG);
  fs.writeFileSync(path.join(root, "src/index.ts"), LIBRARY);
  return root;
}

/** A Markdown file whose fences are the given bodies, with the sources a core would select. */
function markdown(fences: { info?: string; body: string }[]): EmbeddedFile {
  let text = "# Demo\n\n";
  const sources: EmbeddedFile["sources"] = [];
  fences.forEach((fence, index) => {
    text += `\`\`\`${fence.info ?? "ts"}\n`;
    const start = Buffer.byteLength(text);
    text += fence.body;
    sources.push({
      id: `fence-${index}`,
      rule_id: "markdown-code",
      node_pointer: `/children/${index}`,
      source_pointer: `/children/${index}/value`,
      provider: "typescript",
      context: { example: true, info: fence.info ?? "ts" },
      text: fence.body,
      source_range: [start, start + Buffer.byteLength(fence.body)],
    });
    text += "```\n\n";
  });
  return { path: "README.md", hash: "x", text, generated: false, test: false, facts: {}, sources };
}

function index(root: string, host: EmbeddedFile): IndexResult {
  const provider = new Provider("lang-typescript", "0");
  return provider.index({
    project: { root },
    language: "typescript",
    inputs: [{ kind: "regions", regions: host }],
    context: { options: {} },
  });
}

describe("fences of a host file", () => {
  it("use the package around the file as a consumer would", () => {
    const root = project();
    const host = markdown([{ body: 'import { greet } from "demo";\n\ngreet("x");\n' }]);

    const result = index(root, host);

    const fragment = fragmentOf(result);
    const call = fragment.edges!.find((edge) => edge.kind === "calls" && edge.to === GREET);
    assert.deepEqual(call?.site?.start, { line: 6, col: 1 });
    const imported = fragment.edges!.find((edge) => edge.kind === "imports");
    assert.equal(imported?.to, "src/index");
    assert.deepEqual(imported?.site?.start, { line: 4, col: 1 });
    assert.equal(fragment.file.path, "README.md");
    assert.deepEqual(result.incomplete, []);
  });

  it("are modules of their own that the host does not see a wrapper of", () => {
    const root = project();
    const host = markdown([{ body: 'greet("x");\n' }, { body: "export function shown(): number {\n  return 1;\n}\n" }]);

    const fragment = fragmentOf(index(root, host));

    assert.deepEqual(
      fragment.modules!.map((module) => module.path),
      [".lighthouse-snippets/README.md.1/snippet", ".lighthouse-snippets/README.md.2/snippet"],
    );
    const shown = fragment.symbols!.find((symbol) => symbol.name === "shown");
    assert.equal(shown?.file, "README.md");
    assert.deepEqual(shown?.span.start, { line: 8, col: 1 });
    for (const symbol of fragment.symbols!) assert.ok(symbol.span.end.line <= host.text.split("\n").length);
  });

  it("keep identical fences apart", () => {
    const root = project();
    const same = "export const same = 1;\n";

    const fragment = fragmentOf(index(root, markdown([{ body: same }, { body: same }])));

    const ids = fragment.symbols!.map((symbol) => symbol.id);
    assert.deepEqual(ids, [
      ".lighthouse-snippets/README.md.1/snippet::same#const",
      ".lighthouse-snippets/README.md.2/snippet::same#const",
    ]);
  });

  it("degrade to syntactic edges for names the prose defines, with a notice and no gap", () => {
    const root = project();

    const result = index(root, markdown([{ body: 'greet("x");\nundefinedThing();\n' }]));

    const edge = fragmentOf(result).edges!.find((candidate) => candidate.to === "greet");
    assert.equal(edge?.resolution, "syntactic");
    assert.ok(result.notices!.some((notice) => notice.includes("do not type-check")));
    assert.deepEqual(result.incomplete, []);
  });

  it("that are not valid code are a notice, not a gap", () => {
    const root = project();

    const result = index(root, markdown([{ body: "this is { not code\n" }]));

    assert.deepEqual(result.incomplete, []);
    assert.ok(result.notices!.some((notice) => notice.includes("is not valid TypeScript")));
    assert.deepEqual(fragmentOf(result).symbols, []);
  });

  it("take their language from the info string", () => {
    const root = project();

    const result = index(
      root,
      markdown([{ info: "js", body: 'import { greet } from "demo";\nexport const shown = greet(1);\n' }]),
    );

    const fragment = fragmentOf(result);
    assert.equal(fragment.modules![0]!.path, ".lighthouse-snippets/README.md.1/snippet");
    assert.ok(fragment.edges!.some((edge) => edge.kind === "calls" && edge.to === GREET));
  });

  it("map through the indentation of a list item", () => {
    const root = project();
    const body = "export const item = 1;\n";
    const text = `- item\n\n  \`\`\`ts\n  ${body}  \`\`\`\n`;
    const start = Buffer.byteLength("- item\n\n  ```ts\n  ");
    const host: EmbeddedFile = {
      path: "README.md",
      hash: "x",
      text,
      generated: false,
      test: false,
      facts: {},
      sources: [
        {
          id: "f",
          rule_id: "markdown-code",
          node_pointer: "/children/0",
          source_pointer: "/children/0/value",
          provider: "typescript",
          context: { info: "ts" },
          text: body,
          source_range: [start - 2, start + body.length],
          text_map: [{ text_range: [0, body.length], source_range: [start, start + body.length] }],
        },
      ],
    };

    const fragment = fragmentOf(index(root, host));

    const symbol = fragment.symbols!.find((candidate) => candidate.name === "item");
    assert.deepEqual(symbol?.span.start, { line: 4, col: 16 });
  });
});

function fragmentOf(result: IndexResult): Fragment {
  assert.equal(result.fragments.length, 1);
  return result.fragments[0]!;
}
