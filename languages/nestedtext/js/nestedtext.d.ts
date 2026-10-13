import type { NativeLanguage } from "@diaryx/fig";

/** NestedText, compiled from this package's Zig to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const nestedtext: NativeLanguage;
export default nestedtext;
