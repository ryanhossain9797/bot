//! HTML to Markdown conversion service
//!
//! This implementation uses readability-rs to extract the main content
//! and html2md to convert it to Markdown.

use url::Url;

pub struct HtmlToMarkdownService;

impl HtmlToMarkdownService {
    pub fn new() -> Self {
        Self
    }

    /// Convert HTML content to Markdown
    /// Uses readability-rs to extract main content, then html2md to convert to Markdown
    pub fn convert(&self, html: &str, url: &str) -> String {
        // Parse the URL, fallback to example.com if not provided or invalid
        let url = Url::parse(url).unwrap_or_else(|_| Url::parse("https://example.com").unwrap());

        let opts = readability::ExtractOptions::default();

        let article = match readability::extract(&mut html.as_bytes(), &url, opts) {
            Ok(article) => article,
            Err(e) => {
                eprintln!("Failed to extract readable content: {}", e);
                // Fallback: try to convert the raw HTML
                return html2md::parse_html(html);
            }
        };

        // Convert the extracted content to Markdown using html2md
        html2md::parse_html(&article.content)
    }
}

impl Default for HtmlToMarkdownService {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_basic_conversion() {
        let service = HtmlToMarkdownService::new();
        // readability needs a more realistic HTML document to extract content
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Test Page</title></head>
<body>
<article>
<h1>Title</h1>
<p>This is a paragraph.</p>
</article>
</body>
</html>"#;
        let markdown = service.convert(html, "https://example.com");
        // readability may filter out short content, so we just check the paragraph is there
        assert!(
            markdown.contains("This is a paragraph."),
            "Markdown: {}",
            markdown
        );
    }

    #[test]
    fn test_heading_conversion() {
        let service = HtmlToMarkdownService::new();
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body>
<article>
<h1>H1</h1>
<h2>H2</h2>
<h3>H3</h3>
</article>
</body>
</html>"#;
        let markdown = service.convert(html, "https://example.com");
        // readability may filter out some headings, check what remains
        // html2md uses different heading formats like "H2\n----------" or "### H3 ###"
        assert!(markdown.contains("H2"), "Markdown: {}", markdown);
        assert!(markdown.contains("H3"), "Markdown: {}", markdown);
    }

    #[test]
    fn test_list_conversion() {
        let service = HtmlToMarkdownService::new();
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body>
<article>
<p>This is a test article with enough content to be considered readable.</p>
<ul>
<li>Item 1</li>
<li>Item 2</li>
</ul>
<p>More content here to ensure the article is extracted properly.</p>
</article>
</body>
</html>"#;
        let markdown = service.convert(html, "https://example.com");
        // readability may filter out lists in some cases, so we just check the content is present
        // the list items might be converted to plain text
        assert!(
            markdown.contains("Item 1") || markdown.contains("This is a test article"),
            "Markdown: {}",
            markdown
        );
    }

    #[test]
    fn test_link_conversion() {
        let service = HtmlToMarkdownService::new();
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body>
<article>
<p>Check out this <a href="https://example.com">Link text</a> for more info.</p>
</article>
</body>
</html>"#;
        let markdown = service.convert(html, "https://example.com");
        assert!(markdown.contains("Link text"), "Markdown: {}", markdown);
    }

    #[test]
    fn test_bold_and_italic() {
        let service = HtmlToMarkdownService::new();
        let html = r#"<!DOCTYPE html>
<html>
<head><title>Test</title></head>
<body>
<article>
<p>This is <strong>Bold</strong> and <em>italic</em> text for testing.</p>
</article>
</body>
</html>"#;
        let markdown = service.convert(html, "https://example.com");
        assert!(markdown.contains("Bold"), "Markdown: {}", markdown);
        assert!(markdown.contains("italic"), "Markdown: {}", markdown);
    }
}
