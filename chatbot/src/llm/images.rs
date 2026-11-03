use std::sync::Arc;

use super::anthropic::{ContentBlock, ImageSource, Message};

pub const UNDECODABLE: &str =
    "[An image was attached here, but it could not be decoded, so it is not shown.]";

pub fn sanitize(messages: &mut [Message]) {
    for message in messages {
        sanitize_blocks(&mut message.content);
    }
}

fn sanitize_blocks(blocks: &mut [ContentBlock]) {
    for block in blocks {
        let replacement = match block {
            ContentBlock::Image {
                source: ImageSource::Base64 { media_type, data },
            } => match checked(data) {
                Some((mime, bytes)) => {
                    *media_type = mime;
                    *data = bytes;
                    None
                }
                None => Some(ContentBlock::text(UNDECODABLE)),
            },
            ContentBlock::ToolResult { content, .. } => {
                sanitize_blocks(content);
                None
            }
            _ => None,
        };
        if let Some(replacement) = replacement {
            *block = replacement;
        }
    }
}

fn checked(data: &Arc<Vec<u8>>) -> Option<(String, Arc<Vec<u8>>)> {
    let format = image::guess_format(data).ok()?;
    let decoded = image::load_from_memory_with_format(data, format).ok()?;
    if matches!(
        format,
        image::ImageFormat::Png | image::ImageFormat::Jpeg | image::ImageFormat::Gif
    ) {
        return Some((format.to_mime_type().to_string(), Arc::clone(data)));
    }
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(decoded.to_rgb8())
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .ok()?;
    Some(("image/jpeg".to_string(), Arc::new(out.into_inner())))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encoded(format: image::ImageFormat) -> Vec<u8> {
        let mut out = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(8, 8, image::Rgb([9, 99, 199])))
            .write_to(&mut out, format)
            .unwrap();
        out.into_inner()
    }

    fn image_of(message: &Message, at: usize) -> (&str, &[u8]) {
        match &message.content[at] {
            ContentBlock::Image {
                source: ImageSource::Base64 { media_type, data },
            } => (media_type.as_str(), data.as_slice()),
            other => panic!("expected an image, got {other:?}"),
        }
    }

    #[test]
    fn accepted_image_keeps_its_bytes_and_gets_its_real_media_type() {
        let png = encoded(image::ImageFormat::Png);
        let mut messages = vec![Message::user(vec![ContentBlock::image(
            "image/jpeg",
            Arc::new(png.clone()),
        )])];
        sanitize(&mut messages);
        assert_eq!(image_of(&messages[0], 0), ("image/png", png.as_slice()));
    }

    #[test]
    fn formats_native_cannot_decode_are_reencoded_as_jpeg() {
        for (format, mime) in [
            (image::ImageFormat::WebP, "image/webp"),
            (image::ImageFormat::Bmp, "image/bmp"),
        ] {
            let mut messages = vec![Message::user(vec![ContentBlock::image(
                mime,
                Arc::new(encoded(format)),
            )])];
            sanitize(&mut messages);
            let (media_type, bytes) = image_of(&messages[0], 0);
            assert_eq!(media_type, "image/jpeg", "{mime}");
            assert_eq!(image::guess_format(bytes).unwrap(), image::ImageFormat::Jpeg, "{mime}");
        }
    }

    #[test]
    fn undecodable_image_becomes_a_note_even_inside_a_tool_result() {
        let mut messages = vec![Message::user(vec![
            ContentBlock::image("image/heic", Arc::new(vec![1, 2, 3])),
            ContentBlock::ToolResult {
                tool_use_id: "t".into(),
                content: vec![ContentBlock::image("image/png", Arc::new(vec![4]))],
                is_error: false,
            },
        ])];
        sanitize(&mut messages);
        assert!(matches!(&messages[0].content[0], ContentBlock::Text { text } if text == UNDECODABLE));
        let ContentBlock::ToolResult { content, .. } = &messages[0].content[1] else {
            panic!("tool result kept");
        };
        assert!(matches!(&content[0], ContentBlock::Text { text } if text == UNDECODABLE));
    }
}
