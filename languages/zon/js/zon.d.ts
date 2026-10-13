import type { NativeLanguage } from "@diaryx/fig";

/** ZON, Zig Object Notation, compiled from this package's Zig to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const zon: NativeLanguage;
export default zon;
