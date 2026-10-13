import type { NativeLanguage } from "@diaryx/fig";

/** JSON, JSONC and JSON5, compiled from this package's Zig to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const json: NativeLanguage;
export default json;
