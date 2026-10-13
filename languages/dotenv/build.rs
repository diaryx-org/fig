// Compile this package's Zig into `libfig_dotenv.a` for the target being
// built, and link it (`zig build c`; see the kit's `language`).
fn main() {
    fig_language_build::compile("dotenv");
}
