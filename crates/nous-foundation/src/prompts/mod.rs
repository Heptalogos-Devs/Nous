//! Scoped instruction resources, immutable history and bounded MiniJinja rendering.
mod files;
mod render;
mod service;
mod types;
pub use render::validate_template;
pub use service::Prompts;
pub use types::*;
pub const MAX_TEMPLATE_BYTES: usize = 128 * 1024;
const MAX_RENDERED_BYTES: usize = 256 * 1024;
fn digest(text: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(text.as_bytes()))
}
