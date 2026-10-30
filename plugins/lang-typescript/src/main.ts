// lang-typescript is the Lighthouse language plugin for TypeScript and JavaScript.
// It speaks the plugin protocol on stdin and stdout.
import { Provider } from "./provider.ts";
import { serve } from "./server.ts";

const ID = "lang-typescript";
const VERSION = "0.1.0";
const MIN_NODE = 24;

const major = Number(process.versions.node.split(".")[0]);
if (major < MIN_NODE) {
  process.stderr.write(`${ID}: Node ${MIN_NODE} or newer is required, found ${process.versions.node}\n`);
  process.exit(1);
}

const code = await serve(process.stdin, process.stdout, new Provider(ID, VERSION));
if (code !== 0) process.stderr.write(`${ID}: input ended before shutdown and exit\n`);
process.exit(code);
