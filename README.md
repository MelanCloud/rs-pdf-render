# rs-pdf-render
An ultralightweight PDF renderer built on Rust. Built because as a student, I just wanted something to read PDFs with and not have a web browser drink my RAM or Adobe Acrobat's clunk around.

It looks and behaves like Firefox's PDF viewer, minus everything a reader doesn't need:

- A frameless window. The toolbar slides down when the mouse comes near the top edge.
- Zoom buttons and a box to type any zoom, like `150` or `150%`.
- A box to type the page number, next to the page count.
- Rotate counterclockwise.
- The document outline in a sidebar.
- A simple pen with four colors and undo.
- Open and Print in a small menu (the ☰ button). Printing includes the drawings.
- Links in the PDF work: page links jump there, web links open in your browser.
- Back and Forward after following a link or outline entry, or typing a page.
- Each file reopens where you left it, at the same zoom and rotation.
- Password-protected PDFs: it asks for the password, like Firefox.
- Settings (the gear button): light, dark or system theme, smooth scrolling,
  sliding or instant toolbars, always-visible toolbars, the zoom for new files
  (Automatic, Page fit, Page width, 100%) and the scroll speed per wheel notch.
  Smooth scrolling and sliding toolbars start off when Windows' "Animation
  effects" are off.

## Keyboard shortcuts

On macOS, use ⌘ instead of Ctrl.

| Keys | Does |
| --- | --- |
| Ctrl+O | Open a PDF (or drop one on the window) |
| Ctrl+P | Print |
| Ctrl++ / Ctrl+- | Zoom in / out |
| Ctrl+0 | Back to the default zoom |
| Ctrl + mouse wheel | Zoom around the mouse |
| Home / End | First / last page |
| Page Up / Page Down / Space | Scroll a screen |
| Up / Down | Scroll a little |
| Ctrl+Z | Undo the last drawing |
| Ctrl+Shift+Z | Redo |
| Alt+Left / Alt+Right | Back / Forward (also the mouse's side buttons; ⌘[ and ⌘] on macOS) |
| Esc | Close a menu or the password box, or stop drawing |

## Installing on Windows

Run `rs-pdf-render-<version>-setup.exe`. It installs for your user only (no
admin needed; it can also install for everyone), adds a Start menu entry, and
registers the app for PDFs. To make it your default PDF app, tick "Choose it
as the default PDF app" on the last page, or pick it later in Settings → Apps
→ Default apps. Windows doesn't let any installer do that for you. Uninstall
from Settings → Apps; your settings stay in `%APPDATA%\rs-pdf-render`.

The installer isn't signed yet, so Windows may warn "Windows protected your
PC" the first time. Choose "More info", then "Run anyway".

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
   Windows, `lib/libpdfium.dylib` on macOS, `lib/libpdfium.so` on Linux. To
   build the Windows installer, also unpack the whole download into
   `pdfium-win-x64` here, for PDFium's licenses.

3. Run it:

   ```sh
   cargo run --release -- path/to/file.pdf
   ```

For a build you can move around, copy the PDFium library next to
`target/release/rs-pdf-render` (or `rs-pdf-render.exe`). The app looks for it
next to itself first, then in the folder it was started from.

### Building the Windows installer

With [Inno Setup 6](https://jrsoftware.org/isinfo.php) installed (for
example `winget install JRSoftware.InnoSetup`):

```sh
cargo build --release
ISCC installer\rs-pdf-render.iss
```

The installer lands in `target/installer`. The icon is drawn in
`assets/icon.svg`; `assets/icon.ico` and `assets/icon-64.rgba` are made from
it.

## Printing

- **macOS** opens the system print sheet with a preview.
- **Windows** opens the system print dialog, then sends the pages straight to
  the printer.
- **Linux** sends the document to the default printer with `lp`.
