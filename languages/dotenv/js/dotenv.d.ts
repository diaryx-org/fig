import type { NativeLanguage } from "@diaryx/fig";

/** Dotenv, compiled from this package's Zig to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const dotenv: NativeLanguage;
export default dotenv;
