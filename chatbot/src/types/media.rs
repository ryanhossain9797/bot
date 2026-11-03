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
    pub fn normalized(&self) -> Option<Image> {
        let format = image::guess_format(&self.bytes).ok()?;
        let decoded = image::load_from_memory_with_format(&self.bytes, format).ok()?;
        let fits = decoded.width().max(decoded.height()) <= MAX_IMAGE_EDGE;
        let accepted = matches!(
            format,
            image::ImageFormat::Png
                | image::ImageFormat::Jpeg
                | image::ImageFormat::Gif
                | image::ImageFormat::WebP
        );
        if fits && accepted {
            return Some(Image {
                bytes: Arc::clone(&self.bytes),
                mime: format.to_mime_type().to_string(),
            });
        }
        let resized = if fits {
            decoded
        } else {
            decoded.resize(
                MAX_IMAGE_EDGE,
                MAX_IMAGE_EDGE,
                image::imageops::FilterType::Lanczos3,
            )
        };
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(resized.to_rgb8())
            .write_to(&mut out, image::ImageFormat::Jpeg)
            .ok()?;
        Some(Image {
            bytes: Arc::new(out.into_inner()),
            mime: "image/jpeg".to_string(),
        })
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
    pub fn normalized(&self) -> Attachment {
        match self {
            Attachment::Image {
                image: MessageImage::Hydrated(image),
                filename,
                url,
            } => match image.normalized() {
                Some(normalized) => Attachment::Image {
                    image: MessageImage::Hydrated(normalized),
                    filename: filename.clone(),
                    url: url.clone(),
                },
                None => {
                    eprintln!("[media] {filename} ({}) does not decode as an image; passing it on as a file", image.mime);
                    Attachment::File {
                        filename: filename.clone(),
                        content_type: (!image.mime.is_empty()).then(|| image.mime.clone()),
                        url: url.clone(),
                    }
                }
            },
            Attachment::Image { .. } | Attachment::File { .. } => self.clone(),
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

    pub fn hydrated(&self) -> Option<&Image> {
        match self {
            MessageImage::Hydrated(image) => Some(image),
            MessageImage::Dehydrated { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(width: u32, height: u32, format: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(width, height, image::Rgb([9, 99, 199])))
            .write_to(&mut out, format)
            .unwrap();
        out.into_inner()
    }

    fn attachment(bytes: Vec<u8>, mime: &str) -> Attachment {
        Attachment::Image {
            image: MessageImage::Hydrated(Image {
                bytes: Arc::new(bytes),
                mime: mime.to_string(),
            }),
            filename: "pic".to_string(),
            url: "https://cdn/pic".to_string(),
        }
    }

    fn normalized_image(attachment: Attachment) -> Image {
        match attachment.normalized() {
            Attachment::Image {
                image: MessageImage::Hydrated(image),
                ..
            } => image,
            other => panic!("expected a hydrated image, got {other:?}"),
        }
    }

    #[test]
    fn small_accepted_image_keeps_its_bytes_and_gets_its_real_mime() {
        let png = encoded(10, 10, image::ImageFormat::Png);
        let image = normalized_image(attachment(png.clone(), "image/jpeg"));
        assert_eq!(image.mime, "image/png");
        assert_eq!(*image.bytes, png);
    }

    #[test]
    fn large_image_is_downscaled_to_jpeg() {
        let image = normalized_image(attachment(encoded(2048, 512, image::ImageFormat::Png), "image/png"));
        assert_eq!(image.mime, "image/jpeg");
        let decoded = image::load_from_memory(&image.bytes).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (1024, 256));
    }

    #[test]
    fn decodable_but_unaccepted_format_is_reencoded() {
        let image = normalized_image(attachment(encoded(8, 8, image::ImageFormat::Bmp), "image/bmp"));
        assert_eq!(image.mime, "image/jpeg");
    }

    #[test]
    fn undecodable_image_becomes_a_file_attachment() {
        match attachment(vec![1, 2, 3], "image/heic").normalized() {
            Attachment::File {
                filename,
                content_type,
                url,
            } => {
                assert_eq!(filename, "pic");
                assert_eq!(content_type.as_deref(), Some("image/heic"));
                assert_eq!(url, "https://cdn/pic");
            }
            other => panic!("expected a file attachment, got {other:?}"),
        }
    }

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
