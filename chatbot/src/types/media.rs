use serde::ser::SerializeStructVariant;
use serde::{Deserialize, Serialize, Serializer};
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

#[derive(Debug, Clone, Deserialize)]
pub enum MessageImage {
    Hydrated(Image),
    Dehydrated { byte_size: usize },
}

// Persisted state must never carry raw image bytes: always serialize as Dehydrated (size only).
impl Serialize for MessageImage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let byte_size = match self {
            MessageImage::Hydrated(image) => image.bytes.len(),
            MessageImage::Dehydrated { byte_size } => *byte_size,
        };
        let mut variant = serializer.serialize_struct_variant("MessageImage", 1, "Dehydrated", 1)?;
        variant.serialize_field("byte_size", &byte_size)?;
        variant.end()
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hydrated_serializes_as_dehydrated_and_round_trips() {
        let hydrated = MessageImage::Hydrated(Image {
            bytes: Arc::new(vec![1, 2, 3, 4, 5]),
            mime: "image/png".to_string(),
        });
        let json = serde_json::to_string(&hydrated).unwrap();
        assert_eq!(json, r#"{"Dehydrated":{"byte_size":5}}"#);

        let back: MessageImage = serde_json::from_str(&json).unwrap();
        assert!(matches!(back, MessageImage::Dehydrated { byte_size: 5 }));
    }
}
