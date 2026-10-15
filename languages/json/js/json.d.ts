import type { NativeLanguage } from "@diaryx/fig";

/** JSON, JSONC and JSON5, compiled from this package's Rust to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const json: NativeLanguage;
export default json;
