# `NotoSansSC-Subset.ttf` provenance

This is a small, glyph-subsetted derivative of **Noto Sans SC** (a real,
production CJK typeface), used as a test fixture so
`tests/font_embedding_tests.rs` can prove actual CJK glyph *rendering*
fidelity through Pdfium — not just correct PDF/CID structure around a
hand-built, zero-contour synthetic font.

- Upstream font: Noto Sans SC (variable font), from the Google Fonts
  repository:
  <https://github.com/google/fonts/blob/main/ofl/notosanssc/NotoSansSC%5Bwght%5D.ttf>
- License: SIL Open Font License, Version 1.1 (OFL) — see `OFL.txt` in this
  directory, copied unmodified from
  <https://github.com/google/fonts/blob/main/ofl/notosanssc/OFL.txt>.
  Copyright 2014-2021 Adobe, with Reserved Font Name 'Source'/'Noto'.
- How this file was derived (reproducible with `fonttools`, MIT-licensed,
  already a workstation dependency):
  1. Instanced the `wght` variable axis to a single static weight (400):
     `fonttools varLib.instancer -o NotoSansSC-Regular.ttf NotoSansSC[wght].ttf wght=400`
  2. Subset to only the glyphs this test suite actually references, via
     `pyftsubset`:
     `pyftsubset NotoSansSC-Regular.ttf --output-file=NotoSansSC-Subset.ttf --text="中文测试你好世界永" --glyph-names --notdef-glyph --notdef-outline`

The result (~7 KB) keeps real glyph outlines (multi-contour `glyf` data,
not the empty/zero-contour glyphs used by the synthetic fonts built in
`tests/font_embedding_tests.rs`'s `build_test_font` helper) for exactly the
CJK characters exercised by the render test, while staying small enough to
commit to the repository.

The OFL permits distributing modified/subsetted versions of the font
(subsetting is an explicitly anticipated OFL use case) provided the license
text accompanies it and the font is not sold by itself — both conditions
are satisfied here: this is a test fixture bundled with `OFL.txt`, not a
font product.
