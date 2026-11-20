#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.10,<3.14"
# dependencies = ["pikepdf>=9"]
# ///
"""Writes the PDF fixtures of tests/fixtures/pdf: page trees in object
streams or plain objects, inherited and indirect boxes, crop boxes with an
offset origin, incremental updates, and damaged copies."""

import zlib
from pathlib import Path

import pikepdf

OUT = Path(__file__).parent.parent / "fixtures/pdf"


def pages_pdf(sizes, crop=None, inherit=None):
    pdf = pikepdf.new()
    for i, (w, h) in enumerate(sizes):
        page = pdf.add_blank_page(page_size=(w, h))
        if crop and i in crop:
            page.obj.CropBox = pikepdf.Array(crop[i])
    if inherit:
        # Move the first page's MediaBox to the Pages node
        pdf.Root.Pages.MediaBox = pdf.pages[0].obj.MediaBox
        del pdf.pages[0].obj.MediaBox
    return pdf


def save(pdf, name, **kw):
    pdf.save(OUT / name, **kw)


def main():
    OUT.mkdir(exist_ok=True)
    sizes = [(612, 792), (595.28, 841.89), (400, 300), (792, 612)]
    crop = {1: [10, 20, 500.5, 700], 3: [-5, -5, 2000, 2000]}
    for streams in ("generate", "disable"):
        p = pages_pdf(sizes, crop=crop, inherit=True)
        save(
            p,
            f"mixed_{streams}.pdf",
            object_stream_mode=getattr(pikepdf.ObjectStreamMode, streams),
        )
        p = pages_pdf(sizes, crop=crop, inherit=True)
        save(
            p,
            f"mixed_{streams}_uncompressed.pdf",
            object_stream_mode=getattr(pikepdf.ObjectStreamMode, streams),
            compress_streams=False,
            stream_decode_level=pikepdf.StreamDecodeLevel.none,
        )
    # A larger tree (pikepdf balances the page tree only when asked; many
    # pages in one Kids array)
    many = pages_pdf([(300 + i, 400 + 2 * i) for i in range(40)])
    save(many, "many_pages.pdf", object_stream_mode=pikepdf.ObjectStreamMode.generate)
    # Indirect MediaBox array
    p = pages_pdf([(500, 600), (100, 200)])
    p.pages[1].obj.MediaBox = p.make_indirect(pikepdf.Array([0, 0, 321, 123]))
    save(p, "indirect_box.pdf", object_stream_mode=pikepdf.ObjectStreamMode.disable)
    # Incremental update: a newer trailer changes the second page's size
    base = OUT / "mixed_disable.pdf"
    data = base.read_bytes()
    p = pikepdf.open(base)
    p.pages[1].obj.MediaBox = pikepdf.Array([0, 0, 222, 333])
    p.save(
        OUT / "incremental.pdf.tmp", object_stream_mode=pikepdf.ObjectStreamMode.disable
    )
    upd = (OUT / "incremental.pdf.tmp").read_bytes()
    (OUT / "incremental.pdf.tmp").unlink()
    (OUT / "incremental.pdf").write_bytes(data + b"\n" + upd[upd.index(b"1 0 obj") :])
    # Hand-written: page tree in a FlateDecode object stream, inherited box
    objs = [
        (2, b"<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 /MediaBox [0 0 594 756] >>"),
        (3, b"<< /Type /Page /Parent 2 0 R >>"),
        (4, b"<< /Type /Page /Parent 2 0 R /MediaBox 5 0 R >>"),
        (5, b"[0 0 400 300]"),
    ]
    header, body = b"", b""
    for n, t in objs:
        header += b"%d %d " % (n, len(body))
        body += t + b"\n"
    stream = zlib.compress(header + body)
    (OUT / "objstm_handmade.pdf").write_bytes(
        b"%PDF-1.5\n1 0 obj\n<< /Type /Catalog /Pages 2 0 R >>\nendobj\n"
        + b"6 0 obj\n<< /Type /ObjStm /N 4 /First %d /Filter /FlateDecode /Length %d >>\nstream\n"
        % (len(header), len(stream))
        + stream
        + b"\nendstream\nendobj\ntrailer\n<< /Root 1 0 R >>\n%%EOF\n"
    )
    # No page tree, only text boxes (the fallback scan), and nothing at all
    (OUT / "scan_only.pdf").write_bytes(
        b"%PDF-1.4\n/MediaBox [0 0 100.5 200] /MediaBox [ -10 -20 90 80 ] /MediaBox [1.2.3 0 5 5]\n"
    )
    (OUT / "not_a_pdf.pdf").write_bytes(b"not a pdf")
    # Damaged copies: truncated, a corrupted object stream
    src = (OUT / "mixed_generate.pdf").read_bytes()
    (OUT / "truncated_half.pdf").write_bytes(src[: len(src) // 2])
    i = src.index(b"stream") + 40
    (OUT / "corrupt_stream.pdf").write_bytes(src[:i] + bytes(16) + src[i + 16 :])


if __name__ == "__main__":
    main()
