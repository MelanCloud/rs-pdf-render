# rs-pdf-render
An ultralightweight PDF renderer built on Rust. Built because as a student, I just wanted something to read PDFs with and not have a web browser drink my RAM or Adobe Acrobat's clunk around.

It looks and behaves like Firefox's PDF viewer, minus everything a reader doesn't need:

- A frameless window. The toolbar slides down when the mouse comes near the top edge.
- Zoom buttons and a box to type any zoom, like `150` or `150%`.
- A box to type the page number, next to the page count.
- Rotate counterclockwise.
- The document outline in a sidebar.
- A simple pen with four colors and undo.
- Print, with the drawings included.
- Password-protected PDFs: it asks for the password, like Firefox.

## Keyboard shortcuts

On macOS, use ⌘ instead of Ctrl.

| Keys | Does |
| --- | --- |
| Ctrl+O | Open a PDF (or drop one on the window) |
| Ctrl+P | Print |
| Ctrl++ / Ctrl+- | Zoom in / out |
| Ctrl+0 | Automatic zoom |
| Ctrl + mouse wheel | Zoom around the mouse |
| Home / End | First / last page |
| Page Up / Page Down / Space | Scroll a screen |
| Ctrl+Z | Undo the last drawing |
| Esc | Stop drawing, or close the password box |

## Building

You need [Rust](https://rustup.rs) and the PDFium library. The app loads
PDFium when it starts, so it isn't needed to build.

1. Download PDFium for your system from
   [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F8100)
   (release `chromium/8100`). For example, with the GitHub CLI on Windows:

   ```sh
   gh release download chromium/8100 -R bblanchon/pdfium-binaries -p pdfium-win-x64.tgz
   ```

   Use `pdfium-mac-arm64.tgz` on Apple Silicon Macs or `pdfium-linux-x64.tgz` on Linux.

2. Unpack it and copy the library into this folder: `bin/pdfium.dll` on
   Windows, `lib/libpdfium.dylib` on macOS, `lib/libpdfium.so` on Linux.

3. Run it:

   ```sh
   cargo run --release -- path/to/file.pdf
   ```

For a build you can move around, copy the PDFium library next to
`target/release/rs-pdf-render` (or `rs-pdf-render.exe`). The app looks for it
next to itself first, then in the folder it was started from.

## Printing

- **macOS** opens the system print sheet with a preview.
- **Windows** opens the system print dialog, then sends the pages straight to
  the printer.
- **Linux** sends the document to the default printer with `lp`.
