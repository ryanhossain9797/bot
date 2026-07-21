# Document Conversion

## MOBI to EPUB
Use `mobi` library: `pip install mobi` then `mobi.extract("file.mobi")`. Handles conversion in one step. More reliable than calibre's `ebook-convert`.

## PDF to Text
`pip install PyMuPDF` (fast, general) or `pip install pdfplumber` (better for tables).

## General conversion
`pip install pypandoc` then `pypandoc.convert_file("in.ext", "out_format")`. Pandoc covers 100+ formats — your go-to.

## Office files (DOCX, XLSX, PPTX)
- python-docx / python-pptx / openpyxl for reading/writing
- pandoc for DOC to DOCX, DOCX to PDF
- LibreOffice headless: `soffice --headless --convert-to out_format in_file` (complex Office docs)

## Quick reference
- MOBI to EPUB: `mobi.extract()`
- EPUB to PDF: pandoc
- DOCX to PDF: pandoc
- PDF to text: PyMuPDF
- General: pandoc
- Complex Office: LibreOffice
