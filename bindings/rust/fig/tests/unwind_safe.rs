//! `Document` and `Editor` are `UnwindSafe` and `RefUnwindSafe`, as they were
//! when they held the C library's pointers: a caller may hold one across
//! `catch_unwind`.

fn unwind_safe<T: std::panic::UnwindSafe + std::panic::RefUnwindSafe>() {}

#[test]
fn document_and_editor_are_unwind_safe() {
    unwind_safe::<fig::Document>();
    unwind_safe::<fig::Editor>();
}
