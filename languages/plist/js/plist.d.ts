import type { NativeLanguage } from "@diaryx/fig";

/** The Apple XML property list, compiled from this package's Rust to a WebAssembly
 *  module of its own. Pass it to `registerLanguage` from `@diaryx/fig`. */
declare const plist: NativeLanguage;
export default plist;
