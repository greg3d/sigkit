# 08: Raw file import sheet with live preview

**What to build:** A user drops a legacy raw file with no header on the window and a sheet slides up from the bottom with dtype, byte order, header offset in bytes, channel count for interleaved data, rate, gain and offset. The preview re-renders the decoded signal on every change. The starting dtype is guessed by decoding a prefix as each plausible dtype and scoring plausibility. Confirming writes a `header.json` beside the file describing a fixed-rate stream; the data is not copied, and the file then opens like any recording.

**Blocked by:** 05 (Sidebar, panels, overlay, cursor, and multiple recordings)

**Status:** ready-for-agent

- [ ] Dropping a bare column file written by `sigkit gen` (no header) opens the sheet with the correct dtype preselected
- [ ] Every field change re-renders the preview without a confirm step
- [ ] Confirming writes a header beside the file and the file opens as a fixed-rate stream; the raw file's bytes are unchanged
- [ ] An interleaved two-channel raw file imports as a stream of two channels
- [ ] The sheet animates in and out and is dismissed by swipe or Escape; there is no modal dialog
- [ ] A dtype-guess test prefers the generator's true dtype on its own output for each supported dtype
