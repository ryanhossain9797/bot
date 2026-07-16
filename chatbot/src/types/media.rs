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
                .resize(
                    MAX_IMAGE_EDGE,
                    MAX_IMAGE_EDGE,
                    image::imageops::FilterType::Lanczos3,
                )
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
pub enum Attachment {
    Image {
        image: MessageImage,
        filename: String,
        url: String,
    },
    File {
        filename: String,
        content_type: Option<String>,
        url: String,
    },
}

impl Attachment {
    pub fn downscaled(&self) -> Attachment {
        match self {
            Attachment::Image {
                image,
                filename,
                url,
            } => Attachment::Image {
                image: image.downscaled(),
                filename: filename.clone(),
                url: url.clone(),
            },
            Attachment::File { .. } => self.clone(),
        }
    }

    pub fn dehydrated(&self) -> Attachment {
        match self {
            Attachment::Image {
                image,
                filename,
                url,
            } => Attachment::Image {
                image: image.dehydrated(),
                filename: filename.clone(),
                url: url.clone(),
            },
            Attachment::File { .. } => self.clone(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub enum MessageImage {
    Hydrated(Image),
    Dehydrated { byte_size: usize },
}

impl Serialize for MessageImage {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let byte_size = match self {
            MessageImage::Hydrated(image) => image.bytes.len(),
            MessageImage::Dehydrated { byte_size } => *byte_size,
        };
        let mut variant =
            serializer.serialize_struct_variant("MessageImage", 1, "Dehydrated", 1)?;
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
            MessageImage::Dehydrated { byte_size } => MessageImage::Dehydrated {
                byte_size: *byte_size,
            },
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
            MessageImage::Dehydrated { byte_size } => MessageImage::Dehydrated {
                byte_size: *byte_size,
            },
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

    #[test]
    fn image_attachment_persists_dehydrated_keeping_source() {
        let attachment = Attachment::Image {
            image: MessageImage::Hydrated(Image {
                bytes: Arc::new(vec![1, 2, 3]),
                mime: "image/png".to_string(),
            }),
            filename: "photo.png".to_string(),
            url: "https://cdn/photo.png".to_string(),
        };
        let json = serde_json::to_string(&attachment).unwrap();
        let back: Attachment = serde_json::from_str(&json).unwrap();
        match back {
            Attachment::Image { image, filename, url } => {
                assert!(matches!(image, MessageImage::Dehydrated { byte_size: 3 }));
                assert_eq!(filename, "photo.png");
                assert_eq!(url, "https://cdn/photo.png");
            }
            Attachment::File { .. } => panic!("expected image attachment"),
        }
    }

    #[test]
    fn file_attachment_round_trips() {
        let attachment = Attachment::File {
            filename: "report.pdf".to_string(),
            content_type: Some("application/pdf".to_string()),
            url: "https://cdn/report.pdf".to_string(),
        };
        let json = serde_json::to_string(&attachment).unwrap();
        let back: Attachment = serde_json::from_str(&json).unwrap();
        match back {
            Attachment::File { filename, content_type, url } => {
                assert_eq!(filename, "report.pdf");
                assert_eq!(content_type.as_deref(), Some("application/pdf"));
                assert_eq!(url, "https://cdn/report.pdf");
            }
            Attachment::Image { .. } => panic!("expected file attachment"),
        }
    }
}
