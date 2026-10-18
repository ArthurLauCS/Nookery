# Nookery

English | [简体中文](README.zh-CN.md)

Desktop tiles for Windows 11. Loose files on your desktop (folders, documents, pictures, ...) are automatically sorted by type into tiles. Shortcuts stay on the desktop.

- A single ~430 KB exe, 12–14 MB private memory, 0% CPU when idle.
- Five tile opacity levels, down to fully transparent with only a faint border; follows the system light/dark theme.
- Files show the same system icons you see in File Explorer.
- English and Chinese interface.

## Install

Download `Nookery-Setup-<version>.exe` from [Releases](https://github.com/ArthurLauCS/Nookery/releases/latest) and run it. Requires Windows 11.

- The installer is not signed with a commercial code-signing certificate, so Windows may show "Windows protected your PC". Click **More info** → **Run anyway**.
- Setup asks for administrator permission once, to install the certificate for the desktop context menu. If you decline, Nookery still works, but **New tile** is only under **Show more options** in the desktop right-click menu.
- The language you pick in setup is also Nookery's interface language. You can change it later from the tray menu.
- On first run, Nookery tells you how many items it will sort and waits for your confirmation.

## Where your files go

- Sorted files are moved to `Desktop\.Nookery\<tile name>\` (a hidden system folder). They are still inside your desktop folder, so OneDrive desktop sync keeps working.
- Settings are in `%APPDATA%\Nookery\config.txt`: tile positions, lock/collapse state, rules, opacity, pause state.
- Create a folder inside `.Nookery` and it becomes a tile.
- Uninstalling (installed version) stops Nookery, moves every file back to the desktop, and removes autostart, the context menu entry, the certificate, the language setting and the config. If you run the exe directly, click **Restore all to desktop** in the tray menu before deleting it.

## Sorting rules

| Tile | Contents |
|---|---|
| Folders | Folders |
| Documents | doc docx pdf txt md rtf odt xls xlsx csv ppt pptx wps et dps |
| Pictures | png jpg jpeg gif webp bmp svg heic ico psd |
| Videos | mp4 mkv mov avi wmv flv webm |
| Music | mp3 wav flac aac m4a ogg |
| Archives | zip rar 7z tar gz |
| Programs | exe msi |
| Others | Everything else |

Left on the desktop:

- Shortcuts (`.lnk` `.url` `.appref-ms` `.website`)
- Hidden and system files
- Downloads in progress (`.crdownload`, `.part`, ...)
- Office lock files (starting with `~$`)

A file is moved only after it has been unchanged for 3 seconds and can be opened exclusively, and never while you are renaming an icon on the desktop.

- **Change a rule**: drag a file from one tile to another; Nookery asks whether that extension should always go to the new tile.
- **Keep on desktop**: a file dragged from a tile back to the desktop stays there (recorded as `keep` in the config). The record is cleared when the file is deleted or renamed.

## Tile title bar

- The name is centered. Click it to rename; click elsewhere or press Enter to save, Esc to cancel.
- Drag the title bar to move the tile.
- Two buttons at the top right appear on hover:
  - Collapse / expand.
  - Lock / unlock: a locked tile can't be moved or resized, but can still be renamed.
- Locked tiles always show the lock icon; collapsed tiles always show the expand arrow.

## Opacity

Pick it under **Tile opacity** in the tray menu; it applies to all tiles.

| Level | Look |
|---|---|
| 100% / 75% / 50% | Acrylic blur, thinner at lower levels |
| 25% | Translucent, no blur |
| 0% | Fully transparent with a faint border (white outside, black inside, visible on any wallpaper); the background fades in on hover |

At 50% and below, text sits directly on the wallpaper and uses white text with a dark outline, like desktop icons.

## Language

English / Chinese. Switch under **Language** in the tray menu (in Chinese it reads "语言/Language", and the options are "中文/Chinese" and "English", so you can always find your way back). The setting is stored as `Language` under `HKCU\Software\Nookery`; if never set, Nookery follows the Windows display language. After switching:

- Menus, messages and dialogs change immediately.
- Default tiles (Folders, Documents, Pictures, ...) are renamed along with their folders. Tiles you renamed or created are left alone, as is any tile whose new name is already taken.
- **New tile** in the desktop context menu follows the new language.

## Controls

| Action | Result |
|---|---|
| Double-click a file / Enter | Open |
| Arrow keys / Home / End | Move the selection (shows a focus ring) |
| Right-click a file | System context menu |
| F2 | Rename |
| Delete (Shift+Delete) | Move to Recycle Bin (delete permanently) |
| Ctrl+A / C / X / V | Select all / copy / cut / paste |
| Drag out / drag in | Same as File Explorer: move on the same drive, copy across drives; hold Ctrl to copy, Shift to move; the target tile highlights |
| Drag an icon within a tile | Reorder; a line marks where it will land, and the order is remembered |
| Drag This PC, Recycle Bin, etc. from the desktop into a tile | Adds a shortcut to the tile and hides the original desktop icon; the icon comes back once the shortcut leaves every tile (deleted, dragged back to the desktop, tile deleted, restore all, uninstall) |
| Drag the title bar | Move anywhere; snaps within 8 px of screen edges and other tiles, hold Alt to disable snapping |
| Drag an edge | Resize, with the same snapping; minimum height is one row of icons |
| Right-click the title bar or empty space | Open in File Explorer, Delete tile |
| Right-click the desktop → New tile | Creates a tile where you clicked and starts renaming it. In the first-level Windows 11 menu for the installed version; under **Show more options** (or Shift+right-click) when running the exe directly |
| Double-click empty desktop space | Hide / show all tiles, with a hint above the cursor |
| Left-click the tray icon | Hide / show all tiles |
| Right-click the tray icon | New tile, Hide tiles, Double-click desktop to hide tiles (toggle), Pause auto-sorting, Tile opacity, Restore all to desktop, Start with Windows, Language, Exit |

Tiles are rebuilt automatically if File Explorer restarts.

## Build

```bash
cargo build --release
```

Output: `target/release/nookery.exe`. Requires the Rust toolchain and MSVC build tools.

Installer (requires [Inno Setup 6](https://jrsoftware.org/isdl.php) and the Windows SDK):

```bash
powershell -File scripts/installer.ps1
```

Builds release (including the context menu extension in `menu/`), packs `installer/AppxManifest.xml` into a signed sparse package, then builds `target/installer/Nookery-Setup-<version>.exe` using the version in `Cargo.toml`. Signing uses the self-signed `CN=Nookery` certificate in the current user's store, created automatically if missing. The setup script is `installer/nookery.iss`; the Simplified Chinese messages `installer/ChineseSimplified.isl` come from the unofficial translations in the Inno Setup repository.

The app icon sources are `assets/icon.svg` (40 px and up) and `assets/icon-small.svg` (16–32 px). Run `python scripts/icon.py` to regenerate `assets/app.ico` and `assets/app.res` (requires Chrome, Pillow and the Windows SDK); `build.rs` links them into the exe.

Design research and benchmarks are in [docs/调研与方案.md](docs/调研与方案.md) (Chinese).

## Test

```bash
cargo test
```

The scripts in `scripts/` run end-to-end tests on a fake desktop and never touch your real one:

- `setup.ps1`: creates test files in `C:\WorkFile\Temp\fakedesk`; `-Bulk 200` adds 200 more.
- `run.ps1`: starts Nookery with `--root` pointing at the fake desktop, checks sorting, memory and CPU, and takes screenshots.
- `interact.ps1`: sends messages to tile windows via `examples/drive.rs` to test selection, F2 rename, the collapse button, and files dragged back to the desktop.
- `design.ps1 -Round N`: screenshots for design review on a busy fake wallpaper (`backdrop.ps1`), plus grayscale copies:
  - Dark 100 / 75 / 50 / 25 / 0% and light 100%.
  - Locked, collapsed, selected.
  - Renaming, the double-click hint, a second process with `--new-tile`.
- `contrast.ps1`: computes theme contrast against the worst-case background.

Command-line options:

| Option | Purpose |
|---|---|
| `--root <dir>` | Use a directory instead of the desktop; config is stored there too |
| `--top` | Keep tiles topmost, for screenshots |
| `--light` / `--dark` | Force a theme |
| `--new-tile` | New tile at the cursor; forwarded to the running instance if there is one |
| `--uninstall` | Move all files back to the desktop, remove autostart and the context menu, then exit (called by the uninstaller after stopping the running instance); combine with `--root` to test on a fake desktop |

## License

[MIT](LICENSE)
