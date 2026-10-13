import type { NativeLanguage } from "@diaryx/fig";

/** INI, compiled from this package's Zig to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const ini: NativeLanguage;
export default ini;
