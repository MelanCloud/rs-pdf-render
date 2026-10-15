<p align="center">
  <img src="assets/icon.svg" width="96" alt="">
</p>

<h1 align="center">rs-pdf-render</h1>

<p align="center">
  An ultralightweight PDF reader for the desktop, built on Rust.<br>
  Built because, as a student, I just wanted something to read PDFs with and
  not have a web browser drink my RAM or Adobe Acrobat's clunk around.
</p>

<p align="center">
  <img src="docs/screenshot.png" width="640" alt="rs-pdf-render showing a textbook, with the toolbar at the top and the file name at the bottom">
</p>

It looks and behaves like Firefox's PDF viewer, minus everything a reader
doesn't need. The window has no frame: the toolbar slides in when the mouse
nears the top or bottom edge, and gets out of the way while you read.

It never changes your PDFs. Drawings stay on screen only, until you close the
file or print it.

## Download

**Windows 10 and 11 (64-bit):** get `rs-pdf-render-<version>-setup.exe` from
[Releases](https://github.com/MelanCloud/rs-pdf-render/releases) and run it.

- It installs for your user only, so no admin rights are needed. It can also
  install for everyone on the computer.
- To make it your default PDF app, tick "Choose it as the default PDF app" on
  the last page, or pick it later in Settings → Apps → Default apps. Windows
  doesn't let any installer do that by itself.
- The installer isn't code-signed yet, so Windows may say "Windows protected
  your PC". Choose **More info**, then **Run anyway**.
- Prefer no installer? The portable `.zip` on the same page runs from any
  folder. Keep `pdfium.dll` next to the `.exe`.

To uninstall, use Settings → Apps. Your settings stay in
`%APPDATA%\rs-pdf-render` in case you come back.

macOS and Linux work when built from source (see [Building](#building)), but
there are no ready-made packages for them yet.

## Features

- **Reading:** pages render sharp at any zoom, and only the pages near the
  view are kept in memory, so 1000-page textbooks stay light.
- **Zoom:** buttons, a box to type any zoom (`150` or `150%`), Ctrl + mouse
  wheel around the pointer, and pdf.js's zoom modes: Automatic, Page fit,
  Page width and 100%.
- **Pages:** a box to type the page number next to the page count, and
  rotate counterclockwise.
- **Outline:** the document's table of contents in a sidebar. Optionally,
  the app scans the text for numbered subsections like "2.6.4" and adds them,
  for books whose own outline stops at the sections.
- **Links:** page links jump to the right spot, and web and email links open
  in your browser. Back and Forward return you after following a link or an
  outline entry.
- **Picks up where you left off:** each file reopens at the same page, zoom
  and rotation.
- **Pen:** a simple pen with four colors, undo and redo.
- **Print:** the system print dialog, with your drawings included. On
  Windows, text stays sharp because pages go to the printer as vectors.
- **Password-protected PDFs:** it asks for the password, like Firefox.
- **Light, dark or system theme.**
- **Settings** (the gear button): smooth scrolling, sliding or instant
  toolbars, always-visible toolbars, the zoom for new files, the scroll
  speed per wheel notch, and scanning for numbered subsections (off by
  default; what it finds is forgotten when the file closes). Smooth scrolling and sliding toolbars start off if
  Windows' "Animation effects" are off.

## Keyboard and mouse

On macOS, use ⌘ instead of Ctrl.

| Keys | Does |
| --- | --- |
| Ctrl+O | Open a PDF (or drop one on the window) |
| Ctrl+P | Print |
| Ctrl++ / Ctrl+- | Zoom in / out |
| Ctrl+0 | Back to the default zoom |
| Ctrl + mouse wheel | Zoom around the pointer |
| Home / End | First / last page |
| Page Up / Page Down / Space | Scroll a screen |
| Up / Down | Scroll a little |
| Alt+Left / Alt+Right | Back / Forward (also the mouse's side buttons; ⌘[ and ⌘] on macOS) |
| Ctrl+Z / Ctrl+Shift+Z | Undo / redo the last drawing |
| Esc | Close a menu or the password box, or stop drawing |
| Enter | Apply what you typed in the page, zoom or password box |

Drag the top or bottom bar to move the window, and double-click it to
maximize.

## Privacy

The app works offline and sends nothing anywhere. Web links in a PDF open in
your browser only when you click them. The only files it writes are its own
settings and the reading positions of the last 50 files, in
`%APPDATA%\rs-pdf-render` (`~/Library/Application Support/rs-pdf-render` on
macOS, `~/.config/rs-pdf-render` on Linux).

## Building

You need [Rust](https://rustup.rs) and the PDFium library, which the app
loads when it starts.

1. Download PDFium for your system from
   [bblanchon/pdfium-binaries](https://github.com/bblanchon/pdfium-binaries/releases/tag/chromium%2F8100),
   release `chromium/8100`: `pdfium-win-x64.tgz`, `pdfium-mac-arm64.tgz` or
   `pdfium-linux-x64.tgz`. With the GitHub CLI:

   ```sh
   gh release download chromium/8100 -R bblanchon/pdfium-binaries -p pdfium-win-x64.tgz
   ```

2. Unpack it into a folder named after the file (for example
   `pdfium-win-x64/`) inside this repository, and copy the library into the
   repository's top folder: `bin/pdfium.dll` on Windows,
   `lib/libpdfium.dylib` on macOS, `lib/libpdfium.so` on Linux.

3. Run it:

   ```sh
   cargo run --release -- path/to/file.pdf
   ```

The app looks for PDFium next to itself first, then in the folder it was
started from. To move a release build around, copy the library next to
`target/release/rs-pdf-render` (`.exe` on Windows).

### Windows installer

With [Inno Setup 6](https://jrsoftware.org/isinfo.php) installed (for example
`winget install JRSoftware.InnoSetup`) and PDFium unpacked in
`pdfium-win-x64/`:

```sh
cargo build --release
ISCC installer\rs-pdf-render.iss
```

The installer lands in `target/installer/`.

## Printing on each system

- **Windows:** the system print dialog, then pages go straight to the
  printer.
- **macOS:** the system print sheet, with a preview.
- **Linux:** the document goes to the default printer with `lp`.

## Credits

- [PDFium](https://pdfium.googlesource.com/pdfium/) (BSD-3-Clause) renders the
  pages, through [pdfium-render](https://github.com/ajrcarey/pdfium-render) and
  the builds from [pdfium-binaries](https://github.com/bblanchon/pdfium-binaries)
  (MIT). The installer and the portable `.zip` include their licenses in a
  `licenses` folder.
- [iced](https://github.com/iced-rs/iced) (MIT) draws the interface.
- The icons, including the paperclip in the app icon, are from
  [Lucide](https://lucide.dev) (ISC).
- Behavior and colors follow Mozilla's [pdf.js](https://github.com/mozilla/pdf.js).

rs-pdf-render itself is MIT licensed; see [LICENSE](LICENSE).
