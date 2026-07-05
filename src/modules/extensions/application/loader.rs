//! The ExtensionsLoader port itself — a minimal re-export so callers (`app::wire`) import the port and
//! the catalog from `application` alone, never reaching into `infrastructure`.

pub use super::catalog::ExtensionsLoader;