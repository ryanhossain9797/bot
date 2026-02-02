//! HTML to Markdown conversion service
//!
//! This implementation is based on the url2md project by 0yik:
//! <https://github.com/0yik/url2md/blob/main/src/converter/markdown_converter.rs>
//!
//! Licensed under the MIT License - Copyright (c) 2024 0yik
//! See the original repository for full license details.

use regex::Regex;
use scraper::{Html, Selector};

static MULTIPLE_NEWLINES: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();

pub struct HtmlToMarkdownService;

impl HtmlToMarkdownService {
    pub fn new() -> Self {
        Self
    }

    /// Convert HTML content to Markdown
    pub fn convert(&self, html: &str) -> String {
        let document = Html::parse_document(html);
        let mut markdown = String::new();

        // Start from body if present, otherwise from root
        let body_selector = Selector::parse("body").unwrap();
        let root = document.select(&body_selector).next();

        if let Some(body) = root {
            self.process_node(&body, &mut markdown, &document);
        } else {
            // Process from root element
            self.process_node(&document.root_element(), &mut markdown, &document);
        }

        self.clean_markdown(&markdown)
    }

    fn process_node(&self, element: &scraper::ElementRef, output: &mut String, document: &Html) {
        let tag_name = element.value().name.local.as_ref();

        match tag_name {
            "h1" => {
                output.push_str("# ");
                self.process_children(element, output, document);
                output.push('\n');
            }
            "h2" => {
                output.push_str("## ");
                self.process_children(element, output, document);
                output.push('\n');
            }
            "h3" => {
                output.push_str("### ");
                self.process_children(element, output, document);
                output.push('\n');
            }
            "h4" => {
                output.push_str("#### ");
                self.process_children(element, output, document);
                output.push('\n');
            }
            "h5" => {
                output.push_str("##### ");
                self.process_children(element, output, document);
                output.push('\n');
            }
            "h6" => {
                output.push_str("###### ");
                self.process_children(element, output, document);
                output.push('\n');
            }
            "p" => {
                self.process_children(element, output, document);
                output.push_str("\n\n");
            }
            "br" => {
                output.push('\n');
            }
            "hr" => {
                output.push_str("\n---\n");
            }
            "strong" | "b" => {
                output.push_str("**");
                self.process_children(element, output, document);
                output.push_str("**");
            }
            "em" | "i" => {
                output.push('*');
                self.process_children(element, output, document);
                output.push('*');
            }
            "code" => {
                output.push('`');
                self.process_children(element, output, document);
                output.push('`');
            }
            "pre" => {
                output.push_str("```\n");
                self.process_children(element, output, document);
                output.push_str("\n```\n");
            }
            "a" => {
                let href = element.value().attr("href").unwrap_or("");
                output.push('[');
                self.process_children(element, output, document);
                output.push_str(&format!("]({})", href));
            }
            "img" => {
                let src = element.value().attr("src").unwrap_or("");
                let alt = element.value().attr("alt").unwrap_or("");
                output.push_str(&format!("![{}]({})\n", alt, src));
            }
            "ul" => {
                self.process_list(element, output, document, false);
                output.push('\n');
            }
            "ol" => {
                self.process_list(element, output, document, true);
                output.push('\n');
            }
            "li" => {
                // Handled in process_list
                self.process_children(element, output, document);
            }
            "blockquote" => {
                let mut content = String::new();
                self.process_children(element, &mut content, document);
                for line in content.lines() {
                    output.push_str(&format!("> {}\n", line));
                }
            }
            "table" => {
                self.process_table(element, output, document);
            }
            "div" | "span" | "article" | "section" | "main" | "header" | "footer" | "nav"
            | "aside" => {
                // Container elements - just process children
                self.process_children(element, output, document);
            }
            _ => {
                // For other elements, just process children
                self.process_children(element, output, document);
            }
        }
    }

    fn process_children(
        &self,
        element: &scraper::ElementRef,
        output: &mut String,
        document: &Html,
    ) {
        for child in element.children() {
            if let Some(child_element) = scraper::ElementRef::wrap(child) {
                self.process_node(&child_element, output, document);
            } else if let Some(text) = child.value().as_text() {
                output.push_str(text);
            }
        }
    }

    fn process_list(
        &self,
        element: &scraper::ElementRef,
        output: &mut String,
        document: &Html,
        ordered: bool,
    ) {
        let li_selector = Selector::parse("li").unwrap();
        let items: Vec<_> = element.select(&li_selector).collect();

        for (index, item) in items.iter().enumerate() {
            if ordered {
                output.push_str(&format!("{}. ", index + 1));
            } else {
                output.push_str("- ");
            }

            // Process the li content, but avoid double-processing nested lists
            for child in item.children() {
                if let Some(child_element) = scraper::ElementRef::wrap(child) {
                    let tag_name = child_element.value().name.local.as_ref();
                    if tag_name != "ul" && tag_name != "ol" {
                        self.process_node(&child_element, output, document);
                    } else {
                        // Nested list - add newline and indent
                        output.push('\n');
                        let mut nested = String::new();
                        self.process_node(&child_element, &mut nested, document);
                        for line in nested.lines() {
                            output.push_str(&format!("    {}\n", line));
                        }
                    }
                } else if let Some(text) = child.value().as_text() {
                    output.push_str(text);
                }
            }
            output.push('\n');
        }
    }

    fn process_table(&self, element: &scraper::ElementRef, output: &mut String, document: &Html) {
        let tr_selector = Selector::parse("tr").unwrap();
        let th_selector = Selector::parse("th").unwrap();
        let td_selector = Selector::parse("td").unwrap();

        let rows: Vec<_> = element.select(&tr_selector).collect();
        if rows.is_empty() {
            return;
        }

        // Process header row
        let first_row = &rows[0];
        let headers: Vec<_> = first_row.select(&th_selector).collect();

        if !headers.is_empty() {
            output.push('|');
            for header in &headers {
                let mut content = String::new();
                self.process_children(header, &mut content, document);
                output.push_str(&format!(" {} |", content.trim()));
            }
            output.push('\n');

            // Separator
            output.push('|');
            for _ in &headers {
                output.push_str(" --- |");
            }
            output.push('\n');
        }

        // Process data rows
        let start_idx = if headers.is_empty() { 0 } else { 1 };
        for row in &rows[start_idx..] {
            let cells: Vec<_> = row.select(&td_selector).collect();
            if !cells.is_empty() {
                output.push('|');
                for cell in &cells {
                    let mut content = String::new();
                    self.process_children(cell, &mut content, document);
                    output.push_str(&format!(" {} |", content.trim()));
                }
                output.push('\n');
            }
        }
        output.push('\n');
    }

    fn clean_markdown(&self, markdown: &str) -> String {
        let mut result = markdown.to_string();

        // Remove excessive newlines (normalize 3+ newlines to double newline)
        let re = MULTIPLE_NEWLINES.get_or_init(|| Regex::new(r"\n{3,}").unwrap());
        result = re.replace_all(&result, "\n\n").to_string();

        // Trim whitespace
        result.trim().to_string()
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
        let html = "<h1>Title</h1><p>This is a paragraph.</p>";
        let markdown = service.convert(html);
        assert!(markdown.contains("# Title"));
        assert!(markdown.contains("This is a paragraph."));
    }

    #[test]
    fn test_heading_conversion() {
        let service = HtmlToMarkdownService::new();
        let html = "<h1>H1</h1><h2>H2</h2><h3>H3</h3>";
        let markdown = service.convert(html);
        assert!(markdown.contains("# H1"));
        assert!(markdown.contains("## H2"));
        assert!(markdown.contains("### H3"));
    }

    #[test]
    fn test_list_conversion() {
        let service = HtmlToMarkdownService::new();
        let html = "<ul><li>Item 1</li><li>Item 2</li></ul>";
        let markdown = service.convert(html);
        assert!(markdown.contains("- Item 1"));
        assert!(markdown.contains("- Item 2"));
    }

    #[test]
    fn test_link_conversion() {
        let service = HtmlToMarkdownService::new();
        let html = r#"<a href="https://example.com">Link text</a>"#;
        let markdown = service.convert(html);
        assert!(markdown.contains("[Link text](https://example.com)"));
    }

    #[test]
    fn test_bold_and_italic() {
        let service = HtmlToMarkdownService::new();
        let html = "<strong>Bold</strong> and <em>italic</em>";
        let markdown = service.convert(html);
        assert!(markdown.contains("**Bold**"));
        assert!(markdown.contains("*italic*"));
    }
}
