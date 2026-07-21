# Document Conversion

## MOBI to EPUB
Use the `mobi` library's `extract()` function — one step, more reliable than calibre's `ebook-convert`. `pip install mobi`, then:
```python
import mobi
tempdir, filepath = mobi.extract("book.mobi")
# extract() returns (tempdir, filepath); filepath is the unpacked EPUB inside tempdir
import shutil
shutil.copy(filepath, "book.epub")
```

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
