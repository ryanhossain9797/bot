use serde::{Deserialize, Serialize};
use std::sync::Arc;

const MAX_IMAGE_EDGE: u32 = 1024;

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

impl Image {
    pub fn downscaled(&self) -> Image {
        let Ok(decoded) = image::load_from_memory(&self.bytes) else {
            return self.clone();
        };
        if decoded.width().max(decoded.height()) <= MAX_IMAGE_EDGE {
            return self.clone();
        }
        let resized = image::DynamicImage::ImageRgb8(
            decoded
                .resize(MAX_IMAGE_EDGE, MAX_IMAGE_EDGE, image::imageops::FilterType::Lanczos3)
                .to_rgb8(),
        );
        let mut out = std::io::Cursor::new(Vec::new());
        match resized.write_to(&mut out, image::ImageFormat::Jpeg) {
            Ok(()) => Image {
                bytes: Arc::new(out.into_inner()),
                mime: "image/jpeg".to_string(),
            },
            Err(_) => self.clone(),
        }
    }
}

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

    pub fn downscaled(&self) -> MessageImage {
        match self {
            MessageImage::Hydrated(image) => MessageImage::Hydrated(image.downscaled()),
            MessageImage::Dehydrated { byte_size } => {
                MessageImage::Dehydrated { byte_size: *byte_size }
            }
        }
    }
}
