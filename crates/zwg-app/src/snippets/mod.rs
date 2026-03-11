//! Snippet palette state and persistence.

mod store;
mod view;

pub use store::{CsvEncoding, Snippet, SnippetStore};
pub use view::{SnippetPalette, SnippetQueueMode};
