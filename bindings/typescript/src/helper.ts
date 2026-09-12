// A `Language` served to a `fig` command line as a helper process.
//
// The CLI spawns a helper from its `languages.figl`, asks it `describe`
// once, then sends a `parse`, `print` or `render` per call, one JSON line
// each way over stdin and stdout, and the helper answers each on one line.
// `serve` is that loop over `handle` — the same function the wasm module
// calls for a registered language — so a format written for
// `registerLanguage` is a CLI helper with no further code:
//
// ```js
// // dotenv.mjs
// import { serve } from "@diaryx/fig";
// import { dotenv } from "./my-dotenv.mjs";
// await serve(dotenv);
// ```
//
// ```fig
// # ~/.config/fig/languages.figl
// language[]
// > name = js-dotenv
// > extensions = [env]
// > command = [node, ~/.config/fig/languages/dotenv.mjs]
// ```
//
// The defaults are the process's stdin and stdout, so this is Node-only in
// use; but it names no Node type — the input is any async iterable of text
// or bytes, the output any function that takes a line — so the package's
// declarations stay free of `@types/node`, and a test drives it with an
// array.
import { handle, type Language } from "./language.ts";

/** Where `serve` reads requests and writes responses. */
export interface HelperIo {
  /** Request lines, possibly split across chunks; `process.stdin` by default. */
  input?: AsyncIterable<string | Uint8Array>;
  /** Takes one response line (without its newline); `process.stdout` by default. */
  output?: (line: string) => void | Promise<void>;
}

/** Serve `lang` on this process's stdin and stdout until stdin closes. What
 *  a helper script's top level awaits. Every request line is answered, in
 *  order; a blank line is skipped. */
export async function serve(lang: Language, io?: HelperIo): Promise<void> {
  const proc = (globalThis as { process?: { stdin: AsyncIterable<Uint8Array>; stdout: { write(s: string, cb: () => void): unknown } } }).process;
  const input = io?.input ?? proc?.stdin;
  if (!input) throw new Error("serve: no input given and no process.stdin to read");
  const output =
    io?.output ??
    ((line: string) =>
      new Promise<void>((resolve) => {
        proc!.stdout.write(line + "\n", () => resolve());
      }));
  const decoder = new TextDecoder();
  let pending = "";
  for await (const chunk of input) {
    pending += typeof chunk === "string" ? chunk : decoder.decode(chunk, { stream: true });
    let nl: number;
    while ((nl = pending.indexOf("\n")) >= 0) {
      const line = pending.slice(0, nl);
      pending = pending.slice(nl + 1);
      if (line.trim() === "") continue;
      await output(handle(lang, line));
    }
  }
  if (pending.trim() !== "") await output(handle(lang, pending));
}
