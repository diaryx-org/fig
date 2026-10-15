//! fig's errors. A Zig error set is a set of names, and the names are part
//! of fig's contract: the C ABI and the wire map them to statuses
//! (`src/c_api.zig`), and tests expect them by name. So an error here is its
//! Zig name, spelled exactly as Zig spells it: `err!(CannotDeleteSection)`
//! is `error.CannotDeleteSection`. A module's error set is the names it
//! returns, documented where it returns them.

use core::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Error(pub &'static str);

impl Error {
    /// The Zig error name.
    pub const fn name(self) -> &'static str {
        self.0
    }

    pub fn is(self, name: &str) -> bool {
        self.0 == name
    }
}

impl fmt::Debug for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "error.{}", self.0)
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl std::error::Error for Error {}

pub type Result<T> = core::result::Result<T, Error>;

/// `err!(Name)` is the error Zig spells `error.Name`.
#[macro_export]
macro_rules! err {
    ($name:ident) => {
        $crate::error::Error(stringify!($name))
    };
}
