#!/usr/bin/env python3
"""Render a Supernote .note file to per-page transparent ink-layer PNGs.

Thin wrapper over supernotelib (jya-dev/supernote-tool). Emits one PNG per
page plus a JSON manifest on stdout describing what was rendered, so the Rust
ingest agent can do everything else (ink detection, dedup, compositing).

Usage:
    render_note.py INPUT.note OUTPUT_DIR

Output:
    OUTPUT_DIR/page-000.png, page-001.png, ...   (RGBA, ink only, transparent bg)
    stdout: {"pages": [{"index": 0, "path": "...", "template": "<name or null>"}, ...]}
"""

import json
import sys
from pathlib import Path

import numpy as np
import supernotelib as sn
from PIL import Image
from supernotelib.converter import ImageConverter, VisibilityOverlay


def main() -> int:
    if len(sys.argv) != 3:
        print(__doc__, file=sys.stderr)
        return 2

    note_path = Path(sys.argv[1])
    out_dir = Path(sys.argv[2])
    out_dir.mkdir(parents=True, exist_ok=True)

    notebook = sn.load_notebook(str(note_path))
    converter = ImageConverter(notebook)
    # Render ink only: hide the background/template layer so the Rust side can
    # composite our own high-resolution template PNG behind it.
    overlay = sn.converter.build_visibility_overlay(background=VisibilityOverlay.INVISIBLE)

    pages = []
    total = notebook.get_total_pages()
    for i in range(total):
        img = converter.convert(i, overlay)
        # Ink-as-alpha: luminance becomes transparency (white bg -> fully
        # transparent, ink -> opaque black, anti-aliased edges in between),
        # so the page composites cleanly over any template.
        lum = np.asarray(img.convert("L"), dtype=np.uint8)
        rgba = np.zeros((*lum.shape, 4), dtype=np.uint8)
        rgba[..., 3] = 255 - lum
        path = out_dir / f"page-{i:03d}.png"
        Image.fromarray(rgba, "RGBA").save(path)

        template = None
        try:
            page = notebook.get_page(i)
            style = page.get_style()
            # User templates carry the MyStyle file name (e.g. "user_<name>").
            if style is not None:
                template = str(style)
        except Exception:  # noqa: BLE001 - metadata is best-effort
            pass
        pages.append({"index": i, "path": str(path), "template": template})

    json.dump({"pages": pages}, sys.stdout)
    return 0


if __name__ == "__main__":
    sys.exit(main())
