from typing import cast

from PIL import Image, ImageDraw, ImageFont

font = ImageFont.truetype("/usr/share/fonts/Adwaita/AdwaitaMono-Regular.ttf", 13)
ascent, _ = font.getmetrics()
baseline = 12

glyphs = []
for code in range(128):
    rows = [0] * 16
    ch = chr(code)
    if 32 <= code < 127:
        img = Image.new("L", (16, 32), 0)
        draw = ImageDraw.Draw(img)
        draw.text((0, baseline - ascent), ch, font=font, fill=255)
        for y in range(16):
            bits = 0
            for x in range(8):
                pixel = cast(int, img.getpixel((x, y)))
                if pixel >= 110:
                    bits |= 0x80 >> x
            rows[y] = bits
    glyphs.append(rows)

print()
print("const font_w: usize = 8")
print("const font_h: usize = 16")
print()
print("static font: [u8; 2048] = [")
for code, rows in enumerate(glyphs):
    label = repr(chr(code)) if 32 <= code < 127 else f"0x{code:02x}"
    print("    " + ", ".join(f"0x{r:02X}" for r in rows) + f",  // {label}")
print("]")
