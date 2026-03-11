//! Snippet palette state and persistence.

mod settings;
mod store;
mod view;

pub use settings::SnippetSettings;
pub use store::{CsvEncoding, Snippet, SnippetStore};
pub use view::{SnippetPalette, SnippetQueueMode};
