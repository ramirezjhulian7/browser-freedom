"""Genera los iconos de los instaladores a partir del logo (assets/freedom.png).

La marca (el marco abierto y el pájaro, en ámbar) se separa del fondo del logo por color y
queda sobre transparente, como el icono que la ventana dibuja con `Icon::Brand`:

- packaging/icons/freedom.ico: Windows (16 a 256 px).
- packaging/icons/freedom.icns: macOS (16 a 1024 px).
- packaging/icons/png/freedom-<n>.png: Linux (tema hicolor) y la documentación.

Uso (desde la raíz del repositorio; necesita Pillow):

    python tools/gen_icons.py
"""

import os

from PIL import Image

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SOURCE = os.path.join(ROOT, "assets", "freedom.png")
OUT = os.path.join(ROOT, "packaging", "icons")
AMBER = (0xF5, 0xA5, 0x24)
PNG_SIZES = [16, 24, 32, 48, 64, 128, 256, 512, 1024]
ICO_SIZES = [16, 24, 32, 48, 64, 128, 256]


def mark() -> Image.Image:
    """La marca sobre transparente, recortada a un cuadrado con un margen del 4%."""
    src = Image.open(SOURCE).convert("RGB")
    w, h = src.size
    px = src.load()
    # Ámbar contra la tinta del fondo: la diferencia rojo − azul separa una del otro.
    diffs = [px[x, y][0] - px[x, y][2] for y in range(h) for x in range(w)]
    background = sorted(diffs)[len(diffs) // 2]
    strong = sorted(d for d in diffs if d > 90)
    full = strong[int(len(strong) * 0.6)]
    alpha = Image.new("L", (w, h))
    alpha.putdata(
        [max(0, min(255, round((d - background) / (full - background) * 255))) for d in diffs]
    )
    box = alpha.point(lambda a: 255 if a > 128 else 0).getbbox()
    x0, y0, x1, y1 = box
    side = max(x1 - x0, y1 - y0)
    half = side // 2 + round(side * 0.04)
    cx, cy = (x0 + x1) // 2, (y0 + y1) // 2
    alpha = alpha.crop((cx - half, cy - half, cx + half, cy + half))
    out = Image.new("RGBA", alpha.size, AMBER + (0,))
    out.putalpha(alpha)
    return out


def main() -> None:
    os.makedirs(os.path.join(OUT, "png"), exist_ok=True)
    base = mark()
    # El color es uniforme, así que reescalar sin premultiplicar no ensucia los bordes.
    for n in PNG_SIZES:
        base.resize((n, n), Image.LANCZOS).save(
            os.path.join(OUT, "png", f"freedom-{n}.png"), optimize=True
        )
    big = base.resize((1024, 1024), Image.LANCZOS)
    big.save(os.path.join(OUT, "freedom.ico"), sizes=[(n, n) for n in ICO_SIZES])
    big.save(os.path.join(OUT, "freedom.icns"))
    print("iconos en", OUT)


if __name__ == "__main__":
    main()
