import type { NativeLanguage } from "@diaryx/fig";

/** Java .properties, compiled from this package's Rust to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const properties: NativeLanguage;
export default properties;
