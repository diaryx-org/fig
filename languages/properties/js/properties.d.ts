import type { NativeLanguage } from "@diaryx/fig";

/** Java .properties, compiled from this package's Zig to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const properties: NativeLanguage;
export default properties;
