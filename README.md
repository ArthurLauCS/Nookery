# Nookery

English | [简体中文](README.zh-CN.md)

Desktop tiles for Windows 11. Everything on your desktop (shortcuts, folders, documents, pictures, ...) is shown in tiles by type. Files never move: open the Desktop folder in File Explorer and everything is still there.

- A single ~430 KB exe, 12–14 MB private memory, 0% CPU when idle.
- Five tile opacity levels, down to fully transparent with only a faint border; follows the system light/dark theme.
- Files show the same system icons you see in File Explorer.
- English and Chinese interface.

## Install

Download `Nookery-Setup-<version>.exe` from [Releases](https://github.com/ArthurLauCS/Nookery/releases/latest) and run it. Requires Windows 11.

- The installer is not signed with a commercial code-signing certificate, so Windows may show "Windows protected your PC". Click **More info** → **Run anyway**.
- Setup asks for administrator permission once, to install the certificate for the desktop context menu. If you decline, Nookery still works, but **New tile** is only under **Show more options** in the desktop right-click menu.
- The language you pick in setup is also Nookery's interface language. You can change it later from the tray menu.

## Where your files are

- Files stay in your Desktop folder; paths never change. Tiles only display them, so File Explorer, OneDrive desktop sync and recent-file lists all keep working.
- While Nookery runs, File Explorer's own desktop icons are turned off (the same switch as right-click the desktop → **View** → **Show desktop icons**), and everything on the desktop is shown in tiles instead: your files, the shortcuts installers put on the Public desktop, and the system icons (This PC, Recycle Bin, ...) enabled in **Desktop icon settings**. Exiting Nookery turns the desktop icons back on. If Nookery crashes, they stay off until you start it again or turn **Show desktop icons** back on.
- Settings are in `%APPDATA%\Nookery\config.txt`: tile positions, lock/collapse state, rules, which tile each item you placed by hand belongs to (`map` lines), opacity.
- Versions up to 0.2 moved sorted files into `Desktop\.Nookery\<tile name>\`. On first start, Nookery moves them back to the desktop (adding " (2)" on name clashes) and keeps each one in the tile it was in.
- Uninstalling (installed version) stops Nookery, turns the desktop icons back on, and removes autostart, the context menu entry, the certificate, the language setting and the config.

## Sorting rules

| Tile | Contents |
|---|---|
| Shortcuts | lnk url appref-ms website, and system icons like This PC and Recycle Bin |
| Folders | Folders |
| Documents | doc docx pdf txt md rtf odt xls xlsx csv ppt pptx wps et dps |
| Pictures | png jpg jpeg gif webp bmp svg heic ico psd |
| Videos | mp4 mkv mov avi wmv flv webm |
| Music | mp3 wav flac aac m4a ogg |
| Archives | zip rar 7z tar gz |
| Programs | exe msi |
| Others | Everything else |

Not shown in any tile (still in the Desktop folder):

- Hidden and system files
- Downloads in progress (`.crdownload`, `.part`, ...)
- Office lock files (starting with `~$`)
- Files no rule matches, if you deleted the Others tile

New files show up in their tile right away.

- **Place by hand**: drag an item from one tile to another. Nookery remembers the tile (a `map` line in the config); the file itself doesn't move. Renaming a file in a tile keeps it there, even if the new extension belongs to another tile. Renaming it in File Explorer sends it back to the tile its rule picks.
- **Change a rule**: drag files of one type from one tile to another; Nookery asks whether that extension should always go to the new tile.

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
- Default tiles (Shortcuts, Folders, Documents, ...) are renamed. Tiles you renamed or created are left alone, as is any tile whose new name is already taken.
- **New tile** in the desktop context menu follows the new language.

## Controls

| Action | Result |
|---|---|
| Double-click a file / Enter | Open |
| Arrow keys / Home / End | Move the selection (shows a focus ring) |
| Right-click a file | System context menu |
| F2 | Rename |
| Delete (Shift+Delete) | Move to Recycle Bin (delete permanently) |
| Ctrl+A / C / X / V | Select all / copy / cut / paste (pasted files go to the Desktop folder and show up in this tile) |
| Drag between tiles | Moves the item to the other tile; the file stays where it is |
| Drag in from elsewhere | Same as File Explorer: move on the same drive, copy across drives into the Desktop folder; hold Ctrl to copy, Shift to move. The files show up in this tile; the target tile highlights |
| Drag an icon within a tile | Reorder; a line marks where it will land, and the order is remembered |
| Drag This PC, Recycle Bin, etc. (from File Explorer's navigation pane) into a tile | Turns that icon on in **Desktop icon settings** and shows it in this tile |
| Drag the title bar | Move anywhere; snaps within 8 px of screen edges and other tiles, hold Alt to disable snapping |
| Drag an edge | Resize, with the same snapping; minimum height is one row of icons |
| Right-click the title bar or empty space | Open the Desktop folder in File Explorer, Delete tile (its items go to other tiles by the rules) |
| Right-click the desktop → New tile | Creates a tile where you clicked and starts renaming it. In the first-level Windows 11 menu for the installed version; under **Show more options** (or Shift+right-click) when running the exe directly |
| Double-click empty desktop space | Hide / show all tiles, with a hint above the cursor |
| Left-click the tray icon | Hide / show all tiles |
| Right-click the tray icon | New tile, Hide tiles, Double-click desktop to hide tiles (toggle), Tile opacity, Start with Windows, Language, Exit |

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
- `run.ps1`: starts Nookery with `--root` pointing at the fake desktop, checks that files stay put, migration of an old `.Nookery` layout, memory and CPU, and takes screenshots.
- `interact.ps1`: sends messages to tile windows via `examples/drive.rs` to test selection, F2 rename (the file stays in its tile), and the collapse button.
- `design.ps1 -Round N`: screenshots for design review on a busy fake wallpaper (`backdrop.ps1`), plus grayscale copies:
  - Dark 100 / 75 / 50 / 25 / 0% and light 100%.
  - Locked, collapsed, selected.
  - Renaming, the double-click hint, a second process with `--new-tile`.
- `contrast.ps1`: computes theme contrast against the worst-case background.

Command-line options:

| Option | Purpose |
|---|---|
| `--root <dir>` | Use a directory instead of the desktop; config is stored in its `.Nookery` subfolder. Leaves the real desktop icons alone and shows no Public desktop or system icons |
| `--top` | Keep tiles topmost, for screenshots |
| `--light` / `--dark` | Force a theme |
| `--new-tile` | New tile at the cursor; forwarded to the running instance if there is one |
| `--uninstall` | Turn the desktop icons back on, move files left by versions up to 0.2 back to the desktop, remove autostart and the context menu, then exit (called by the uninstaller after stopping the running instance) |

## License

[MIT](LICENSE)
