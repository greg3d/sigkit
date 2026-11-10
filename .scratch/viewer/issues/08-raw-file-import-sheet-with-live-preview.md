# 08: Raw file import sheet with live preview

**What to build:** A user drops a legacy raw file with no header on the window and a sheet slides up from the bottom with dtype, byte order, header offset in bytes, channel count for interleaved data, rate, gain and offset. The preview re-renders the decoded signal on every change. The starting dtype is guessed by decoding a prefix as each plausible dtype and scoring plausibility. Confirming writes a `header.json` beside the file describing a fixed-rate stream; the data is not copied, and the file then opens like any recording.

**Blocked by:** 05 (Sidebar, panels, overlay, cursor, and multiple recordings)

**Status:** in-progress

- [x] Dropping a bare column file written by `sigkit gen` (no header) opens the sheet with the correct dtype preselected
- [ ] Every field change re-renders the preview without a confirm step
- [x] Confirming writes a header beside the file and the file opens as a fixed-rate stream; the raw file's bytes are unchanged
- [x] An interleaved two-channel raw file imports as a stream of two channels
- [ ] The sheet animates in and out and is dismissed by swipe or Escape; there is no modal dialog
- [x] A dtype-guess test prefers the generator's true dtype on its own output for each supported dtype

## Comments

Implemented. Verified by tests (`sigkit-format/tests/raw_guess.rs`, `raw_import.rs`) and by `--screenshot`: a `sigkit gen raw --dtype i16 --channels 2 --byte-order big` file opens the sheet guessed as i16 big-endian, 2 channels, with the preview drawn; with its header written it reopens as a two-channel fixed-rate stream, raw bytes unchanged. Still to check by hand: field changes redraw the preview live; the sheet slides in and out; swipe down and Escape dismiss it; clicking Import opens the recording.

The guess also picks byte order and channel count. The header gains a `raw` block (file, header offset, byte order) for reading a file in place; the pyramid cache is now version 2, since it fingerprints that layout. Limits: one raw file per folder (the header is always `header.json`), and the guess reads from byte 0, ignoring a header offset typed later.
