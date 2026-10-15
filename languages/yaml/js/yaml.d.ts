import type { NativeLanguage } from "@diaryx/fig";

/** YAML, compiled from this package's Rust to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const yaml: NativeLanguage;
export default yaml;
