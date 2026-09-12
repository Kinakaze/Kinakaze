//! Native module loading through operating-system export tables.

mod module;
mod module_image;
pub mod native;
mod sdk;

pub use module::{Export, ExportKind, Module, ModuleLifecycle, ModuleSet};
pub use module_image::ModuleImage;
pub use sdk::{ImportLibraryInfo, generate_import_library, inspect_import_library};

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug)]
pub struct Error(String);

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

impl From<std::io::Error> for Error {
    fn from(value: std::io::Error) -> Self {
        Self(value.to_string())
    }
}

pub(crate) fn invalid(message: impl Into<String>) -> Error {
    Error(message.into())
}
