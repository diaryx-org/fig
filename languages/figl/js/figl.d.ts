import type { NativeLanguage } from "@diaryx/fig";

/** Figl, fig's authoring dialect, compiled from this package's Rust to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const figl: NativeLanguage;
export default figl;
