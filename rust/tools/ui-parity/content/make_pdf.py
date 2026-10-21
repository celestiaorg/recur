# Writes page.pdf: one page with an embedded Inter face (no standard-font substitution), text,
# rules and filled shapes. Run with reportlab installed: python make_pdf.py
from pathlib import Path
from reportlab.lib.colors import HexColor
from reportlab.pdfbase import pdfmetrics
from reportlab.pdfbase.ttfonts import TTFont
from reportlab.pdfgen import canvas

here = Path(__file__).parent
fonts = here.parent.parent.parent / "crates/ui/assets/fonts"
pdfmetrics.registerFont(TTFont("Inter", str(fonts / "Inter-400.ttf")))
pdfmetrics.registerFont(TTFont("InterSemi", str(fonts / "Inter-600.ttf")))
c = canvas.Canvas(str(here / "page.pdf"), pagesize=(420, 300), invariant=1)
c.setFillColor(HexColor("#1f2933"))
c.setFont("InterSemi", 22)
c.drawString(32, 250, "Quarterly report")
c.setFont("Inter", 11)
lines = [
    "Rendering stays identical across the web and native apps.",
    "Pages are drawn at the device scale and laid out to fit the width.",
]
for i, line in enumerate(lines):
    c.drawString(32, 222 - i * 16, line)
c.setStrokeColor(HexColor("#9aa5b1"))
c.setLineWidth(1)
c.line(32, 186, 388, 186)
for i, (h, col) in enumerate([(60, "#e6a817"), (96, "#6cb6ff"), (40, "#7ee08a"), (120, "#e0837d")]):
    c.setFillColor(HexColor(col))
    c.rect(48 + i * 84, 40, 56, h, stroke=0, fill=1)
c.setFillColor(HexColor("#1f2933"))
c.circle(360, 140, 18, stroke=0, fill=1)
c.showPage()
c.save()
