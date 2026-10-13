import type { NativeLanguage } from "@diaryx/fig";

/** TOML, compiled from this package's Zig to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const toml: NativeLanguage;
export default toml;
