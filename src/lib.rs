#![feature(windows_by_handle)]
#![deny(missing_docs)]
#![deny(missing_debug_implementations)]
#![cfg_attr(not(test), warn(unused_crate_dependencies))]
#![cfg_attr(test, deny(warnings))]
#![cfg_attr(docsrs, feature(doc_cfg))]

//! Wakthrough crate.
#[cfg(feature = "async")]
#[cfg_attr(docsrs, doc(cfg(feature = "async")))]
pub mod r#async;
mod error;
mod sync;

pub use error::{Error, Result};
pub use sync::{DirEntry, WalkDir, Walker};
