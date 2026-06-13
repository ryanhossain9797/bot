use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize)]
pub struct Image {
    pub bytes: Arc<Vec<u8>>,
    pub mime: String,
}

impl std::fmt::Debug for Image {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Image")
            .field("mime", &self.mime)
            .field("bytes", &format_args!("{} bytes", self.bytes.len()))
            .finish()
    }
}

/// An image that is either carrying its bytes (live, fed to the model) or has been
/// reduced to its size (persisted in history — never holds bytes).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum MessageImage {
    Hydrated(Image),
    Dehydrated { byte_size: usize },
}

impl MessageImage {
    pub fn dehydrated(&self) -> MessageImage {
        match self {
            MessageImage::Hydrated(image) => MessageImage::Dehydrated {
                byte_size: image.bytes.len(),
            },
            MessageImage::Dehydrated { byte_size } => {
                MessageImage::Dehydrated { byte_size: *byte_size }
            }
        }
    }

    pub fn hydrated_bytes(&self) -> Option<Arc<Vec<u8>>> {
        match self {
            MessageImage::Hydrated(image) => Some(Arc::clone(&image.bytes)),
            MessageImage::Dehydrated { .. } => None,
        }
    }
}
