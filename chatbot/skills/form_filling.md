# Form Filling

For filling a **flat** PDF (or image) form — one with no interactive form fields, just a printed layout — by stamping text onto it at coordinates. The tool is **PyMuPDF** (`import fitz`), already installed. (If a PDF *does* have real interactive fields, use those instead: `page.widgets()` / `widget.field_value`. This skill is for the common case where it doesn't.)

The entire job is finding the right `(x, y)` for each field. Do it **empirically** with a render-and-look loop — never guess all coordinates blind and fill in one shot.

## Process

### 1. Look at the form first
Rasterize the page to a PNG and actually view it before doing anything:
```python
import fitz
doc = fitz.open("form.pdf")
doc[0].get_pixmap(dpi=150).save("page.png")   # then open and look at page.png
```
Coordinates you'll write are in **PDF points** (72 per inch), not image pixels. Handy trick: also render one at `dpi=72` — then 1 image pixel ≈ 1 PDF point, so an `(x, y)` you eyeball in that PNG maps almost directly to the number you pass to `insert_text`. (Read small text off the 150-dpi render; estimate coordinates off the 72-dpi one.)

### 2. List the fields
Enumerate every spot that needs input — give each a short `field_id`, its human label, and roughly where it sits. Write this list down first. You're **mapping** the form here, not filling it.

### 3. Locate each field by trial and error
For each field, stamp a **placeholder value** (e.g. the field's own name) at a best-guess `(x, y)`, render, and look. Nudge `x`/`y`/`fontsize` until it lands cleanly inside the right box.
```python
page.insert_text((x, y), "SAMPLE", fontsize=9.5, fontname="helv", color=(0, 0, 0))
```
`insert_text` anchors at the text **baseline**; the page origin is top-left with `y` growing downward. Don't overthink the coordinate math — the look-and-adjust loop converges in a couple of tries.

### 4. Build a coordinates JSON as you go
Persist each solved field the moment it's right, so progress survives a context reset. Keep it in your workspace (`/work/coordinates.json`):
```json
{
  "page_index": 0,
  "fields": [
    { "field_id": "full_name", "label": "Full name", "x": 214, "y": 225, "fontsize": 9.5, "fontname": "helv" }
  ]
}
```
Note this holds only **locations**, not values yet.

### 5. Verify each field before moving to the next
After placing a field, re-render and confirm **that** field sits correctly before starting the next one. Strictly one at a time — a pile of unverified coordinates is a pile of mistakes you'll have to untangle later.

### 6. Ask the user for the real values LAST
Only once **every** field's location is nailed down, ask the user for the actual values — all at once, as a checklist of the labels you discovered. Drop the values into the JSON, render each into place (still verifying), and save the final PDF:
```python
doc.save("/work/filled.pdf")
```

## Checkboxes and marks
No text — draw the mark inside the box:
```python
r = fitz.Rect(x0, y0, x1, y1)              # the checkbox bounds
page.draw_line((r.x0, r.y0), (r.x1, r.y1))  # an X
page.draw_line((r.x1, r.y0), (r.x0, r.y1))
```
Or just `insert_text` an `"X"` / `"✓"` at the box.

## Tips
- `fontname`: `"helv"` (Helvetica), `"helvB"` (bold), `"cour"` (mono).
- `color`: `(0,0,0)` black; many forms are filled in blue `(0,0,1)`.
- Text colliding with the pre-printed label → nudge `y` or lower `fontsize`.
- Long values overflowing a field → shrink `fontsize`, or split across lines with a second `insert_text`.
- Multi-page forms → repeat per page via `doc[page_index]`.
