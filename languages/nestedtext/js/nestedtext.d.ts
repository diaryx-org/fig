import type { NativeLanguage } from "@diaryx/fig";

/** NestedText, compiled from this package's Rust to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const nestedtext: NativeLanguage;
export default nestedtext;
