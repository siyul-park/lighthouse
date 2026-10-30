import fs from "node:fs";
import path from "node:path";
import ts from "typescript";
import { selfReference } from "./project/selfref.ts";

const CONFIG_NAMES = ["tsconfig.json", "jsconfig.json"];

/** The options of the program a file in no project gets: JavaScript allowed, not checked, ESNext, bundler resolution. */
const DEFAULT_OPTIONS: ts.CompilerOptions = {
  target: ts.ScriptTarget.ESNext,
  module: ts.ModuleKind.ESNext,
  moduleResolution: ts.ModuleResolutionKind.Bundler,
  allowJs: true,
  checkJs: false,
  jsx: ts.JsxEmit.Preserve,
  strict: true,
  esModuleInterop: true,
  resolveJsonModule: true,
  allowImportingTsExtensions: true,
  skipLibCheck: true,
};

/** A config file with the files it lists and the projects it refers to. */
interface Config {
  path: string;
  parsed: ts.ParsedCommandLine;
  files: Set<string>;
}

/** One program of the request: what it was made from and the files it analyzes. */
export interface Built {
  /** The tsconfig or jsconfig that governs the program; undefined for the default program. */
  config: string | undefined;
  program: ts.Program;
  host: ts.CompilerHost;
  /** Notices about the configuration. */
  notices: string[];
}

/** Finds the project of a file and builds one program per project, reading overlays instead of the disk. */
export class Workspace {
  private readonly configs = new Map<string, Config | null>();

  constructor(
    readonly root: string,
    private readonly overlays: ReadonlyMap<string, string>,
  ) {}

  /** The config that includes the file, looking up from its directory and through project references. */
  ownerOf(file: string): string | undefined {
    for (const dir of this.ancestors(file)) {
      for (const name of CONFIG_NAMES) {
        const found = this.includes(path.join(dir, name), file, new Set());
        if (found !== undefined) return found;
      }
    }
    return undefined;
  }

  /** The nearest config above a file, whether it lists the file or not. */
  nearestConfig(file: string): string | undefined {
    for (const dir of this.ancestors(file)) {
      for (const name of CONFIG_NAMES) {
        const candidate = path.join(dir, name);
        if (this.config(candidate) !== undefined) return this.listing(candidate, file);
      }
    }
    return undefined;
  }

  /** Builds the program of a config, or the default program when the config is undefined. */
  build(config: string | undefined, files: readonly string[]): Built {
    const notices: string[] = [];
    let options: ts.CompilerOptions = DEFAULT_OPTIONS;
    let roots = [...files];
    let references: readonly ts.ProjectReference[] | undefined;
    const governing = config === undefined ? undefined : this.config(config);
    if (governing !== undefined) {
      const { parsed } = governing;
      options = { ...parsed.options };
      roots = [...new Set([...parsed.fileNames, ...files])];
      references = parsed.projectReferences;
      for (const diagnostic of parsed.errors) {
        notices.push(
          `${this.display(governing.path)}: ${ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n")}`,
        );
      }
    }
    options = {
      ...options,
      allowJs: true,
      noEmit: true,
      composite: false,
      incremental: false,
      declaration: false,
      declarationMap: false,
      emitDeclarationOnly: false,
      tsBuildInfoFile: undefined,
      skipLibCheck: true,
      // The protocol owns stdout: a project's tracing options must not print to it.
      traceResolution: false,
    };
    const host = this.host(options);
    const program = ts.createProgram({ rootNames: roots, options, host, projectReferences: references });
    return { config: governing === undefined ? undefined : config, program, host, notices };
  }

  private host(options: ts.CompilerOptions): ts.CompilerHost {
    const host = ts.createCompilerHost(options, true);
    const { readFile, fileExists, getSourceFile } = host;
    const overlays = this.overlays;
    host.getCurrentDirectory = () => this.root;
    host.trace = undefined;
    host.readFile = (file) => overlays.get(file) ?? readFile.call(host, file);
    host.fileExists = (file) => overlays.has(file) || fileExists.call(host, file);
    host.getSourceFile = (file, languageVersionOrOptions, onError, shouldCreate) => {
      const text = overlays.get(file);
      return text === undefined
        ? getSourceFile.call(host, file, languageVersionOrOptions, onError, shouldCreate)
        : ts.createSourceFile(file, text, languageVersionOrOptions, true);
    };
    (
      host as ts.CompilerHost & { useSourceOfProjectReferenceRedirect?(): boolean }
    ).useSourceOfProjectReferenceRedirect = () => true;
    const cache = ts.createModuleResolutionCache(this.root, (file) => file, options);
    host.resolveModuleNameLiterals = (literals, containing, redirect, resolutionOptions, containingFile) =>
      literals.map((literal) => {
        const mode = ts.getModeForUsageLocation(containingFile, literal, resolutionOptions);
        const resolved = ts.resolveModuleName(literal.text, containing, resolutionOptions, host, cache, redirect, mode);
        if (resolved.resolvedModule !== undefined) return resolved;
        const own = selfReference(containing, literal.text, (file) => host.fileExists(file));
        return own === undefined ? resolved : { resolvedModule: own };
      });
    return host;
  }

  /** The directories from a file's own up to the root. */
  private ancestors(file: string): string[] {
    const dirs: string[] = [];
    let dir = path.dirname(file);
    while (dir.startsWith(this.root)) {
      dirs.push(dir);
      if (dir === this.root) break;
      dir = path.dirname(dir);
    }
    return dirs;
  }

  private includes(configPath: string, file: string, seen: Set<string>): string | undefined {
    if (seen.has(configPath)) return undefined;
    seen.add(configPath);
    const config = this.config(configPath);
    if (config === undefined) return undefined;
    if (config.files.has(file)) return configPath;
    for (const reference of config.parsed.projectReferences ?? []) {
      const found = this.includes(ts.resolveProjectReferencePath(reference), file, seen);
      if (found !== undefined) return found;
    }
    return undefined;
  }

  /** The config that lists the file or, for a solution-style config that lists none, the first reference above the file. */
  private listing(configPath: string, file: string): string {
    const config = this.config(configPath);
    if (config === undefined || config.files.size > 0) return configPath;
    for (const reference of config.parsed.projectReferences ?? []) {
      const referenced = ts.resolveProjectReferencePath(reference);
      if (file.startsWith(path.dirname(referenced) + "/")) return referenced;
    }
    return configPath;
  }

  private config(configPath: string): Config | undefined {
    let config = this.configs.get(configPath);
    if (config === undefined) {
      config = this.parse(configPath);
      this.configs.set(configPath, config);
    }
    return config ?? undefined;
  }

  private parse(configPath: string): Config | null {
    if (!fs.existsSync(configPath)) return null;
    const host: ts.ParseConfigFileHost = {
      ...ts.sys,
      onUnRecoverableConfigFileDiagnostic: () => undefined,
    };
    const parsed = ts.getParsedCommandLineOfConfigFile(configPath, {}, host);
    return parsed === undefined ? null : { path: configPath, parsed, files: new Set(parsed.fileNames) };
  }

  private display(file: string): string {
    return path.relative(this.root, file).split(path.sep).join("/");
  }
}
