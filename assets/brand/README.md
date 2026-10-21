# VisiConvert identity

The mark pairs two incoming blocks with a forward arrow: inquiries becoming an
outcome. Its solid geometry, cut corners, and open gaps follow the VisiGrid and
VisiBooks family. Keep the gaps and proportions unchanged.

- `visiconvert-mark.svg`: dark mark on transparent background.
- `visiconvert-mark-light.svg`: light mark for dark backgrounds.
- `visiconvert-mark-clay.svg`: warm clay version for the marketing site.
- `visiconvert-icon.svg` and `visiconvert-icon-light.svg`: square application icons.
- `visiconvert-wordmark.svg` and `visiconvert-wordmark-light.svg`: outlined,
  font-independent horizontal wordmarks.
- `favicon.svg`: adapts to the browser's light/dark preference.

Use at least 16 px of height and leave clear space equal to one quarter of the
mark's height. Dark ink is `#101014`, light ink `#F5F6F8`, and clay `#C35132`.
The wordmark uses Manrope at weights 800 and 500; its font license is retained
in `FONT-LICENSE.txt`. These SVGs contain paths, not external fonts or images.

Run `python3 scripts/sync-brand.py` after editing assets to copy the web variants
into the application and, when present, the Astro site.
