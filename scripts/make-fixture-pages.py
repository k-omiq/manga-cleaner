#!/usr/bin/env python3
"""Draw the smoke-test pages.

These are **not** a quality corpus and must never be used as one. Manga109 is
licence-gated and the A/B needs real scans; what these are for is the
questions a synthetic page can honestly answer:

- does the detector produce boxes at all, in roughly the right places
- does the segmentation mask come back registered to the page
- does the script gate leave the English caption alone
- does the ring sampling tell flat paper from screentone
- does peak RSS stay flat across a page with tens of regions

Every single page is 1600×2400, the worked-example dimensions, and
each one isolates a case:

    page-dialogue    two balloons of vertical Japanese, an English caption
    page-screentone  a balloon over 60 lpi halftone, which flat fill must refuse
    page-sfx         a large free-floating sound effect over art
    page-noisy       the dialogue page with scan grain, which rung 1 exists for
    page-crowded     twenty detected regions (47 drawn), for Phase 3's memory criterion

And three pages that are one webtoon strip, 800×4000 each - the dimensions
`src/lib/editor/strip.js` assumes:

    strip-01  a wide balloon straddling the first content-minimum split point
    strip-02  the other half of a balloon cut by the page join at strip y=4000
    strip-03  a page whose first eight rows repeat strip-02's last eight

**What the strip fixture does and does not answer.** It constructs two of the
three exit-criteria cases - the bubble split across a page join, and the
bubble at a content-minimum split point - and it constructs one verified join
and one anomalous one for the join check. It says nothing at all about the
third criterion, peak RSS under 400 MB on a 200-page webtoon: three synthetic
pages are not a 200-page chapter, and these files must not stand in for a
corpus.

**What the crowded fixture does and does not answer.** It draws 47 text
regions across planar fill, screentone, and sound effects across the 512²
tiling and 2048² decline thresholds, but the detector emits only
20 of them. It lets a harness measure whether per-region resident memory is
freed between those 20 detected regions or accumulated across the page. It does
not answer whether inpainting over screentone, tile boundaries, or decline
thresholds work end to end: the detector emits no region on any of the four
screentone panels, only one detected region tiles (splitting degenerately into
two tiles whose origins are 14 pixels apart, sharing 498 of their 512 rows, which
does not exercise a tile boundary), and the >2048 px region is never detected so
the decline-past-four-model-inputs path has never run on a real page. Like
every other synthetic page here, it says nothing about inpainting quality on
real manga art.

Run: scripts/make-fixture-pages.py [outdir]
"""

import math
import random
import sys
from pathlib import Path

from PIL import Image, ImageDraw, ImageFont

W, H = 1600, 2400
PAPER = 246          # cream-white, not 255: avoids the near-white snap
INK = 20

JP_FONT = "/System/Library/Fonts/ヒラギノ角ゴシック W3.ttc"
JP_FONT_HEAVY = "/System/Library/Fonts/ヒラギノ角ゴシック W8.ttc"
EN_FONT = "/System/Library/Fonts/HelveticaNeue.ttc"


def font(path, size, index=0):
    return ImageFont.truetype(path, size, index=index)


def vertical_text(draw, x, y, text, fnt, fill=INK, leading=1.08):
    """One column of vertical Japanese, drawn glyph by glyph.

    PIL has no vertical writing mode, so the column is composed by hand. That is
    fine for a fixture: what the detector sees is glyph blobs in a column, which
    is what real vertical text is.
    """
    step = int(fnt.size * leading)
    for i, ch in enumerate(text):
        draw.text((x, y + i * step), ch, font=fnt, fill=fill)
    return y + len(text) * step


def balloon(draw, cx, cy, rx, ry, tail=None):
    draw.ellipse((cx - rx, cy - ry, cx + rx, cy + ry), fill=PAPER, outline=INK, width=6)
    if tail:
        draw.polygon(tail, fill=PAPER, outline=INK)
        # Re-draw the ellipse edge over the tail's join so it reads as one shape.
        draw.ellipse((cx - rx + 4, cy - ry + 4, cx + rx - 4, cy + ry - 4), fill=PAPER)


def screentone(img, box, pitch=12, radius=3.2, angle=45.0):
    """A halftone dot field: a downscale averages this to flat
    grey, sending the region to a flat fill and putting a grey rectangle inside
    dotted tone."""
    draw = ImageDraw.Draw(img)
    x0, y0, x1, y1 = box
    a = math.radians(angle)
    ca, sa = math.cos(a), math.sin(a)
    steps_u = int((x1 - x0 + y1 - y0) / pitch) + 2
    for iu in range(-steps_u, steps_u):
        for iv in range(-steps_u, steps_u):
            u, v = iu * pitch, iv * pitch
            px = x0 + u * ca - v * sa
            py = y0 + u * sa + v * ca
            if x0 <= px <= x1 and y0 <= py <= y1:
                draw.ellipse((px - radius, py - radius, px + radius, py + radius), fill=60)


def panel(draw, box, width=5):
    draw.rectangle(box, outline=INK, width=width)


def new_page():
    img = Image.new("L", (W, H), PAPER)
    return img, ImageDraw.Draw(img)


def page_dialogue():
    img, d = new_page()
    panel(d, (80, 80, W - 80, 1100))
    panel(d, (80, 1160, W - 80, H - 80))

    jp = font(JP_FONT, 52)
    balloon(d, 500, 400, 230, 300)
    vertical_text(d, 560, 190, "なにを", jp)
    vertical_text(d, 470, 190, "しているの", jp)

    balloon(d, 1150, 800, 210, 240)
    vertical_text(d, 1190, 640, "まって", jp)
    vertical_text(d, 1100, 640, "くれ", jp)

    balloon(d, 700, 1700, 260, 260)
    vertical_text(d, 760, 1520, "そんな", jp)
    vertical_text(d, 670, 1520, "ばかな", jp)

    # English, which the gate must leave alone.
    en = font(EN_FONT, 38)
    d.text((140, H - 200), "Chapter 1 - translated by nobody", font=en, fill=INK)
    d.text((140, H - 150), "SCANLATION GROUP", font=en, fill=INK)
    return img


def page_screentone():
    img, d = new_page()
    panel(d, (80, 80, W - 80, H - 80))
    screentone(img, (100, 100, W - 100, H - 100))
    d = ImageDraw.Draw(img)

    jp = font(JP_FONT, 56)
    balloon(d, 800, 900, 300, 340)
    vertical_text(d, 870, 660, "きこえる", jp)
    vertical_text(d, 780, 660, "だれか", jp)

    # A second balloon sitting on tone, near the first, so the two share a
    # decode window - the reason engine context comes from the running
    # composite rather than the raw page.
    balloon(d, 800, 1600, 240, 260)
    vertical_text(d, 850, 1460, "いない", jp)
    return img


def page_sfx():
    img, d = new_page()
    panel(d, (80, 80, W - 80, H - 80))
    screentone(img, (100, 1200, W - 100, H - 100), pitch=16, radius=5.0, angle=15.0)
    d = ImageDraw.Draw(img)

    # Art: a few strokes, so the sound effect is over something.
    for i in range(14):
        d.line((120 + i * 100, 200, 400 + i * 60, 1100), fill=INK, width=3)

    heavy = font(JP_FONT_HEAVY, 240)
    vertical_text(d, 700, 300, "ドドド", heavy, leading=1.0)
    heavy_small = font(JP_FONT_HEAVY, 150)
    vertical_text(d, 1150, 1400, "ゴゴゴ", heavy_small, leading=1.0)

    jp = font(JP_FONT, 48)
    balloon(d, 420, 1800, 220, 240)
    vertical_text(d, 460, 1670, "にげろ", jp)
    return img


def page_noisy():
    """The dialogue page under scan grain.

    Rung 1 exists for "tight masks on noisy or JPEG scans", and
    every other page here is noiseless - their measured floor is 0.0, so every
    region routes to rung 0 and rung 1 never runs at all. This is the fixture
    that makes it run.

    The grain is normal rather than uniform, at roughly the amplitude of a
    consumer flatbed scan. Uniform grain over a handful of levels is *bimodal*
    by the ring's Otsu test - two halves five levels apart and one and a half
    wide - so a uniform fixture would be refused as multimodal and routed to the
    ladder instead.
    """
    img = page_dialogue()
    rng = random.Random(20260813)
    pixels = img.load()
    for y in range(H):
        for x in range(W):
            v = pixels[x, y] + rng.gauss(0.0, 2.6)
            pixels[x, y] = 0 if v < 0 else 255 if v > 255 else int(round(v))
    return img


def page_crowded():
    """A page packed with 47 drawn text regions (20 detected) across ladder rungs.

    The Phase 3 memory exit criterion requires peak RSS to stay flat
    across a multi-region page, and no existing fixture
    tests this because the earlier pages contain only a few regions each.

    The generator draws 47 regions mixing clean dialogue balloons (planar fill,
    rung 0), vertical text over halftone screentone (intended for rung 2), and
    free-floating sound effects over art, including four drawn regions larger than
    512 px on one side and one larger than 2048 px. Under the detector, only 20
    regions are actually emitted: the vertical text over screentone is never
    detected on any of the four screentone panels, only one detected region tiles
    (splitting degenerately into two tiles whose origins are 14 pixels apart,
    sharing 498 of their 512 rows, which does not exercise a tile boundary), and
    the single region drawn larger than 2048 px is never detected, so the
    decline-past-four-model-inputs path has never run end to end on a real page.
    """
    img, d = new_page()

    # Left full-height panel: action speedlines and giant SFX (>2048 px)
    panel(d, (60, 60, 240, 2340))
    for i in range(8):
        d.line((70 + i * 18, 70, 120 + i * 12, 2330), fill=INK, width=2)

    # 4 tiers x 3 columns of panels across the rest of the canvas
    tiers = [
        (60, 600),
        (630, 1170),
        (1200, 1760),
        (1790, 2340),
    ]
    cols = [
        (270, 660),
        (690, 1100),
        (1130, 1540),
    ]

    for y0, y1 in tiers:
        for x0, x1 in cols:
            panel(d, (x0, y0, x1, y1))

    # Screentones on selected panels (inpaint ladder)
    screentone(img, (1140, 70, 1530, 590), pitch=12, radius=3.0, angle=45.0)
    screentone(img, (280, 640, 650, 1160), pitch=14, radius=3.5, angle=30.0)
    screentone(img, (700, 1210, 1090, 1750), pitch=12, radius=3.2, angle=45.0)
    screentone(img, (280, 1800, 650, 2330), pitch=15, radius=4.0, angle=60.0)
    d = ImageDraw.Draw(img)

    # Speedlines on SFX panels
    for i in range(8):
        d.line((290 + i * 45, 70, 330 + i * 40, 590), fill=INK, width=2)
    for i in range(8):
        d.line((1150 + i * 45, 640, 1200 + i * 40, 1160), fill=INK, width=2)
    for i in range(8):
        d.line((710 + i * 45, 1800, 760 + i * 40, 2330), fill=INK, width=2)

    # Fonts
    jp_m = font(JP_FONT, 42)
    jp_tall = font(JP_FONT, 46)
    heavy_s = font(JP_FONT_HEAVY, 90)
    heavy_m = font(JP_FONT_HEAVY, 95)
    heavy_m2 = font(JP_FONT_HEAVY, 100)
    heavy_l = font(JP_FONT_HEAVY, 135)
    heavy_xl = font(JP_FONT_HEAVY, 160)

    # Panel 0: Left strip - giant SFX > 2048 px
    vertical_text(d, 65, 80, "ゴオオオオオオオオオオオオオ", heavy_xl, leading=1.0)

    # Panel 1: Tier 1, Col A - SFX (>512 px)
    vertical_text(d, 280, 75, "ドドドド", heavy_l, leading=1.0)
    vertical_text(d, 460, 100, "ドン", heavy_m2, leading=1.0)
    vertical_text(d, 460, 360, "ザッ", heavy_s, leading=1.0)

    # Panel 2: Tier 1, Col B - Clean Dialogue
    balloon(d, 770, 200, 65, 120)
    vertical_text(d, 750, 120, "どこへ", jp_m)
    balloon(d, 990, 200, 75, 120)
    vertical_text(d, 970, 110, "いくのか", jp_m)
    balloon(d, 770, 450, 65, 120)
    vertical_text(d, 750, 370, "やめろ", jp_m)
    balloon(d, 990, 450, 75, 120)
    vertical_text(d, 970, 360, "あぶない", jp_m)

    # Panel 3: Tier 1, Col C - Screentone text
    vertical_text(d, 1170, 100, "きこえる", jp_m)
    vertical_text(d, 1265, 100, "だれかのこえ", jp_m)
    vertical_text(d, 1360, 100, "とおくから", jp_m)
    vertical_text(d, 1455, 100, "よんでいる", jp_m)
    vertical_text(d, 1220, 420, "たすけて", jp_m)
    vertical_text(d, 1370, 420, "はやく", jp_m)

    # Panel 4: Tier 2, Col A - Screentone text
    vertical_text(d, 310, 680, "くらやみのなか", jp_m)
    vertical_text(d, 400, 680, "ひかりがみえた", jp_m)
    vertical_text(d, 490, 680, "あしおとがする", jp_m)
    vertical_text(d, 580, 680, "ちかづいてくる", jp_m)

    # Panel 5: Tier 2, Col B - Clean Dialogue
    balloon(d, 770, 770, 65, 120)
    vertical_text(d, 750, 690, "しずかに", jp_m)
    balloon(d, 990, 770, 75, 120)
    vertical_text(d, 970, 680, "みつかるぞ", jp_m)
    balloon(d, 770, 1020, 65, 120)
    vertical_text(d, 750, 940, "ここから", jp_m)
    balloon(d, 990, 1020, 75, 120)
    vertical_text(d, 970, 930, "にげるんだ", jp_m)

    # Panel 6: Tier 2, Col C - SFX (>512 px)
    vertical_text(d, 1150, 645, "ズズズズ", heavy_l, leading=1.0)
    vertical_text(d, 1330, 680, "ガッ", heavy_m, leading=1.0)
    vertical_text(d, 1330, 960, "バン", heavy_m, leading=1.0)

    # Panel 7: Tier 3, Col A - Clean Dialogue (>512 px tall balloon)
    balloon(d, 350, 1480, 65, 270)
    vertical_text(d, 328, 1215, "そんなばかなことがある", jp_tall)
    balloon(d, 540, 1340, 75, 110)
    vertical_text(d, 520, 1260, "うそだろ", jp_m)
    balloon(d, 540, 1600, 75, 110)
    vertical_text(d, 520, 1520, "ほんとうだ", jp_m)

    # Panel 8: Tier 3, Col B - Screentone text
    vertical_text(d, 740, 1250, "かぜのねがする", jp_m)
    vertical_text(d, 830, 1250, "あめがふってきた", jp_m)
    vertical_text(d, 920, 1250, "さむくなってきた", jp_m)
    vertical_text(d, 1010, 1250, "いそがなければ", jp_m)

    # Panel 9: Tier 3, Col C - Clean Dialogue
    balloon(d, 1210, 1340, 65, 115)
    vertical_text(d, 1190, 1260, "まにあった", jp_m)
    balloon(d, 1430, 1340, 75, 115)
    vertical_text(d, 1410, 1260, "よかった", jp_m)
    balloon(d, 1210, 1590, 65, 115)
    vertical_text(d, 1190, 1510, "つぎへいこう", jp_m)
    balloon(d, 1430, 1590, 75, 115)
    vertical_text(d, 1410, 1510, "ついてきて", jp_m)

    # Panel 10: Tier 4, Col A - Screentone text
    vertical_text(d, 310, 1840, "あさやけのそら", jp_m)
    vertical_text(d, 400, 1840, "まちがみえる", jp_m)
    vertical_text(d, 490, 1840, "もうすぐつく", jp_m)
    vertical_text(d, 580, 1840, "がんばろう", jp_m)

    # Panel 11: Tier 4, Col B - SFX (>512 px)
    vertical_text(d, 705, 1800, "ゴゴゴゴ", heavy_l, leading=1.0)
    vertical_text(d, 890, 1830, "カッ", heavy_m2, leading=1.0)
    vertical_text(d, 890, 2100, "キィ", heavy_m, leading=1.0)

    # Panel 12: Tier 4, Col C - Clean Dialogue
    balloon(d, 1210, 1920, 65, 115)
    vertical_text(d, 1190, 1840, "おわりだ", jp_m)
    balloon(d, 1430, 1920, 75, 115)
    vertical_text(d, 1410, 1840, "かえろう", jp_m)
    balloon(d, 1210, 2180, 65, 115)
    vertical_text(d, 1190, 2100, "またあした", jp_m)
    balloon(d, 1430, 2180, 75, 115)
    vertical_text(d, 1410, 2100, "さようなら", jp_m)

    return img


# ---------------------------------------------------------------------------
# The longstrip fixture
# ---------------------------------------------------------------------------

SW = 800           # strip width, and every page's width
SPH = 4000         # page height; strip.js assumes 800×4000
SPAGES = 3
SH = SPH * SPAGES  # strip height, 12 000


def hatch(draw, box, pitch=9, angle=32.0, width=2):
    """Dense diagonal art: a band of rows with high edge energy.

    Rule 2 scores rows by gradient energy, so the fixture needs
    something for the quiet rows to be quiet *against*. Hatching is the cheapest
    honest way to make a row loud without it being text.
    """
    x0, y0, x1, y1 = box
    span = (x1 - x0) + (y1 - y0)
    step = pitch / max(math.cos(math.radians(angle)), 1e-3)
    n = int(span / step) + 2
    for i in range(-n, n):
        x = x0 + i * step
        draw.line((x, y0, x + (y1 - y0) * math.tan(math.radians(angle)), y1),
                  fill=INK, width=width)


def horizontal_lines(draw, x, tops, text, fnt):
    """Horizontal Japanese, one line per entry in `tops`.

    Horizontal rather than the vertical columns the single pages use, and that
    is the whole point of this fixture: a *horizontal* split through a column of
    vertical text lands in the balloon's top or bottom margin, outside the
    detector's box, and cuts nothing. Webtoon balloons set text in horizontal
    lines, so the quiet rows are the **inter-line gaps** - inside the merged
    text region, which is what puts a bubble at the split point rather than
    beside it.
    """
    for top in tops:
        draw.text((x, top), text, font=fnt, fill=INK)


def rounded_balloon(draw, box, radius=90):
    draw.rounded_rectangle(box, radius=radius, fill=PAPER, outline=INK, width=6)


def strip_canvas():
    """Draw the whole strip in **strip coordinates**, then slice it.

    The canvas is a convenience of the generator and not a claim about the
    application: rule 1 refuses a full-strip buffer in the *cleaner*,
    and the only way to guarantee that the join at y=4000 is a genuine
    continuous cut - no duplicated rows, no misregistration, no seam - is to cut
    it out of continuous content.
    """
    img = Image.new("L", (SW, SH), PAPER)
    d = ImageDraw.Draw(img)
    jp = font(JP_FONT, 60)

    # --- Page 1 -----------------------------------------------------------
    hatch(d, (0, 0, SW, 1300))

    # Case (b): a balloon at the first content-minimum split point.
    #
    # The first split targets strip y=2000 with a ±500 band, so the search runs
    # over 1500–2500. Below 2010 that band is the inside of this balloon, and
    # nothing is hatched **beside** it: an interior row crosses two outline
    # strokes and nothing else, which is what puts it under rule 2's acceptance
    # floor of 10% of the page median. Above 2010 the band is dense hatching, so
    # the quietest row in it is the first inter-line gap, at y≈1500 - inside the
    # balloon, and above three of its four lines of text.
    #
    # Hatching the margins beside the balloon instead is the version that does
    # **not** work, and it is worth recording why: 120 px of art either side of
    # a 680 px balloon is 15% of a hatched row's energy, so every interior row
    # scored 9.6 against a page median of 64 and the whole band failed the floor
    # - the split fell back to a hard cut at exactly 2000 and the fixture
    # constructed nothing.
    # Hatched first and cleared beside the balloon afterwards, rather than
    # hatched around it. A gap of even eight unhatched rows between the
    # balloon's bottom and the next band of art is a full-width row of blank
    # paper, which scores zero and wins the band outright - the split then lands
    # just *below* the bubble instead of inside it.
    hatch(d, (0, 1300, SW, 3620))
    d.rectangle((0, 1330, 59, 2010), fill=PAPER)
    d.rectangle((741, 1330, SW, 2010), fill=PAPER)
    rounded_balloon(d, (60, 1340, 740, 2010))
    horizontal_lines(d, 110, [1395, 1555, 1715, 1875], "そんなことが", jp)

    # Case (a): a balloon straddling the page join at strip y=4000, with a line
    # of text sitting across the cut. The join is continuous content, so
    # `strip::check_join` verifies it and rule 1 lets the read cross.
    d.ellipse((100, 3700, 700, 4300), fill=PAPER, outline=INK, width=6)
    horizontal_lines(d, 190, [3820, 3960, 4100], "きこえるか", jp)

    # --- Page 2 -----------------------------------------------------------
    hatch(d, (0, 4400, SW, 5700))
    rounded_balloon(d, (90, 5800, 710, 6400))
    horizontal_lines(d, 140, [5870, 6030, 6190], "まってくれ", jp)
    # Right up to the page edge, so the duplicated rows below carry content:
    # `strip::check_join` refuses to call a repeated blank margin a duplicate.
    hatch(d, (0, 6500, SW, 8000))

    # --- Page 3 -----------------------------------------------------------
    hatch(d, (0, 8000, SW, 9300))
    rounded_balloon(d, (90, 9400, 710, 10000))
    horizontal_lines(d, 140, [9470, 9630, 9790], "だれもいない", jp)
    hatch(d, (0, 10100, SW, SH))
    return img


def strip_pages():
    """The strip, sliced into pages, with one join deliberately broken.

    Join 1 (pages 1→2) is a clean cut: the join check finds no duplicated
    rows and no misregistration, so rule 1 verifies it and a read
    crosses it. Join 2 (pages 2→3) repeats eight rows, which is the
    "duplicated overlap rows" case - the one join anomaly the i18n catalogue has a
    sentence for (`notice.input.joinAnomaly`).
    """
    strip = strip_canvas()
    pages = [strip.crop((0, i * SPH, SW, (i + 1) * SPH)) for i in range(SPAGES)]
    pages[2].paste(pages[1].crop((0, SPH - 8, SW, SPH)), (0, 0))
    return {f"strip-{i + 1:02d}": page for i, page in enumerate(pages)}


PAGES = {
    "page-dialogue": page_dialogue,
    "page-screentone": page_screentone,
    "page-sfx": page_sfx,
    "page-noisy": page_noisy,
    "page-crowded": page_crowded,
}


if __name__ == "__main__":
    out = Path(sys.argv[1] if len(sys.argv) > 1 else "fixtures/pages")
    out.mkdir(parents=True, exist_ok=True)
    built = {name: build() for name, build in PAGES.items()}
    built.update(strip_pages())
    for name, image in built.items():
        path = out / f"{name}.png"
        # Grayscale, 8-bit: the mode most manga raws actually are, and the one
        # the old exporter promoted to RGB.
        image.save(path, optimize=True)
        print(f"wrote {path} ({path.stat().st_size // 1024} KB)")
