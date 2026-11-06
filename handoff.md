# AryaCADStudio — Electrical CAD handoff

State of the work, what another machine needs to build it, and where the sharp edges are.
Written 2026-11-05. Repo has **14 commits**; one file is uncommitted (see *Current state*).

---

## 1. What this is

`AryaCADStudio` is a fork of **Open CAD Studio 2026.44** (Rust, `iced` 0.15-dev, wgpu,
~300k LOC). On top of it sits an **electrical CAD layer** modelled on SkyCAD and
AutoCAD Electrical: a symbol library, placement, tagging, wiring, terminals, cross
references, PLC I/O, project files and reports.

Two directories matter:

| Path | What it is |
|---|---|
| `C:\Users\HomeAdmin\AryaCode\KonnectSuite\AryaCADStudio` | the code (git repo, ~33 GB incl. `target/`) |
| `C:\Users\HomeAdmin\AryaCode\KonnectSuite\AyraCADElectrical` | the *data*: library, fixtures, real drawings, reports |

The data folder is not in the repo. Copy it separately.

```
AyraCADElectrical\
  library\        51 symbol DXFs + library.tsv (symbol, class, attributes, sheets, scale)
  tests\          ~70 fixture DXFs and command batches (see §6)
  import\Prints\DXF\   34 real sheets (FC1000495-xxx) - plot exports, no attributes
  reports\        ecad-verification.md + screenshots
  tools\          harvest_library.py, make_*_test.py, DXF readers
```

---

## 2. What must be installed to build

Verified on the machine this was written on:

| Component | Version used | Notes |
|---|---|---|
| **Rust** (`rustup`) | **1.99.1** stable | toolchain `stable-x86_64-pc-windows-msvc` |
| **cargo** | 1.99.1 | comes with rustup |
| **Visual Studio 2022 Build Tools** | MSVC **14.44.35207** | needs the *Desktop development with C++* workload |
| **Windows 10/11 SDK** | **10.0.26100.0** | the MSVC linker needs it |
| **Git** | any recent | the repo history is the only record of the work |
| **GPU driver** | DX12-capable | the app runs `[gpu] backend: dx12`; Intel HD 530 worked |

Install with:

```powershell
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e `
  --override "--quiet --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then confirm:

```powershell
rustup default stable-x86_64-pc-windows-msvc
rustc -vV    # host: x86_64-pc-windows-msvc
```

**Not needed:** Node/pnpm, Python, .NET SDK, Docker, WSL.
Python 3 *is* used by the helper scripts in `tools/` (harvesting symbols, reading DXFs
back for verification), but never by the build.

### Build

```powershell
cd C:\Users\HomeAdmin\AryaCode\KonnectSuite\AryaCADStudio
$env:PATH = "$env:USERPROFILE\.cargo\bin;$env:PATH"
cargo build --release -j 4
```

- **Incremental: ~12 minutes.** **Clean: 35–40 minutes** (this crate is huge; be patient).
- Output: `target\release\OpenCADStudio.exe` (~150 MB).
- Run: `.\target\release\OpenCADStudio.exe` — add a drawing path to open one.

Two settings in the repo exist because this crate is at the edge of what the compiler
likes:

- `.cargo/config.toml` sets `RUST_MIN_STACK = 67108864` ("rustc 1.99 overflows the
  default 8 MB compiling this crate in release").
- `Cargo.toml` sets `[profile.dev.package.meshopt] debug = 0`. **With `debug = 1` every
  dev build fails** (MSVC `cl D8050: cannot execute c1xx.dll`). Release builds were never
  affected.

---

## 3. Current state (read this before building)

- Committed and working: everything through **`e3fe933cf` "ELECSHEET: drawing frame and
  title block"**.
- **Uncommitted:** `src/app/commands/elec.rs` — a rewrite of `ELECSHEET` so the frame fits
  the drawing's own extent instead of being drawn at 297×210 mm in units where the content
  is only ~35×25. `cargo check --release` passed at the time of writing; the full build was
  interrupted, so **this change is unverified**. Either finish it or `git checkout` it.

```
git status          # one modified file
git log --oneline   # 14 commits
```

---

## 4. The electrical commands

All live in `src/app/commands/elec.rs` (~4,200 lines) and dispatch through
`dispatch_elec`. Domain logic is in `crates/elec` (3,366 lines, dependency-free, **121 unit
tests**).

| Command | Does |
|---|---|
| `ELECLIB <folder>` | load a folder of symbol DXFs (+ `library.tsv`); remembers the path in `%APPDATA%\AryaCAD\elec_library.txt` |
| `ELECPALETTE` | modal symbol palette with drawn thumbnails |
| `ELECINSERT` / `ELECPLACE <symbol>` | place a library symbol, class-based tagging |
| `ELECSETATTR <tag> k=v,...` | edit attributes at the library's label height |
| `ELECWIRE` | polyline wire with junction dots |
| `ELECJUNCTIONS` | place junction dots where wires meet |
| `ELECTERMINALS`, `ELECTSIDES` | terminal strips, side A/B assignment |
| `ELECXREF`, `ELECXREFWRITE` | cross references between parts of one device |
| `ELECLIST`, `ELECBOM [csv]`, `ELECWIRELIST`, `ELECREPORT <xlsx>` | listings and reports |
| `ELECNUMBER` / `ELECNUMBERPREVIEW` | tag devices on the current drawing |
| `ELECNEW [template.dxf]` | new drawing, optional template |
| `ELECMAKESYMBOL <name>` | turn selected geometry into a library symbol |
| `ELECPROJECT <folder> [out.xlsx]` | scan every sheet, project BOM workbook |
| `ELECPROJNEW/OPEN/LIST` | project file (`project.eprj`, plain text) |
| `ELECPROJNUM [apply]` | renumber devices across the project (preview by default, `.dxf.bak` backup) |
| `ELECPLC <tag> <points> <base> [x,y]` | place a PLC module with `ADDR01..ADDRnn` |
| `ELECPLCNEXT <tag> <points> [prefix] [x,y]` | continue I/O addressing across sheets |
| `ELECIO` | per-point cross references (address → device on that conductor) |
| `ELECPANEL <out.dxf>` | panel arrangement drawing |
| `ELECSHEET [A4|A3|A2|A1] [title]` | frame + title block (see the caveat in §7) |

---

## 5. Project file format

`project.eprj`, plain text, relative paths, hand-editable:

```
# AryaCAD electrical project
sheet sheet-01.dxf
number 1
title Feeder panel
sheet sheet-02.dxf
number 2
title Motor starter
```

`number` and `title` belong to the `sheet` line above them. The project file is the single
source of truth for sheet membership and order — `ELECPROJECT` (no argument), `ELECPLCNEXT`
and `ELECPROJNUM` all read it.

---

## 6. How to verify anything (do not skip this)

The app is a GUI, but every command runs headless:

```powershell
$exe = "target\release\OpenCADStudio.exe"
& $exe --each tests\e2e_batch.txt tests\e2e_test.dxf
```

- `--each <batch.txt> <drawing.dxf>` runs one command per line, then exits.
- Blank line = Enter (ends the current interactive command).
- Point input is `x,y` on one line — **not** separate x and y lines (silently ignored).
- `--script <file> <drawing>` runs a script at GUI startup.
- Read results back **from the saved DXF**, not from the command's own output; the
  Python readers in `tests\` (`read_addrs.py`, `read_tags.py`, `read_panel.py`) do this.

Batches worth knowing: `e2e_batch.txt` (full draft + reports), `plc_batch.txt`,
`io6_batch.txt`, `pn3.txt` (project numbering), `meta2_batch.txt` (metadata + workbook),
`sheet_batch.txt`.

---

## 7. Known problems / sharp edges

1. **`ELECSHEET` scale (in progress).** Drawing frames at true paper size does not fit
   drawings whose content is ~35×25 units; symbols end up microscopic. The uncommitted
   change fits the frame to the drawing extent.
2. **`ELECPANEL` has no footprints** — fixed 14×10 boxes, no dimensions, no DIN rail.
   Ordering is alphabetical by tag.
3. **Library is thin** — 51 symbols, all authored symbols get class `Symbol` (`-S1`).
4. **Your exported sheets carry no component attributes** — they live only on `WD_M`, so
   project BOMs come out empty on them (24 components across 34 sheets).
5. **App-level tests not re-run since a source-corruption incident** (§8). `crates/elec`
   tests pass; `cargo test --release` for the app has not been verified end to end.
6. **Single-instance edit lock** — a second session cannot write a drawing the GUI has
   open, so headless batches fail on files the app is showing.

---

## 8. The incident that must not repeat

`src/app/commands/elec.rs` once reached **1.56 GB**, with a single **736-million-character
line**: a PowerShell `Get-Content`/`Set-Content` round trip had mojibake-doubled a doc
comment, and each later edit doubled it again. That one literal inflated the crate's
artifacts — the library rlib hit **4.65 GB with 4.19 GB of metadata**, over the 4 GiB
archive-member limit — and every binary failed to link with:

```
error[E0786]: corrupt metadata encountered in libOpenCADStudio-*.rlib
```

Hours were spent treating it as a compiler limit (`codegen-units`, `opt-level`,
pipelining, `-j 1`, full clean) before the real cause was found.

**Rules from that:**

- **Never** edit source files with PowerShell `Set-Content`, `-replace`, or `>`. Use the
  file-editing tool, or a Python script that writes bytes exactly once.
- If a file grows unexpectedly or a build fails with E0786, check the largest source file
  first: `Get-ChildItem src -Recurse -Include *.rs | Sort-Object Length -Descending | Select -First 5`.
- `src/lib.rs.disabled`, `src/main_lib_linked.rs.disabled` and
  `src/bin/ocs_launcher.rs.disabled` are leftovers from the workaround; the real crate
  layout (`lib.rs` + thin `main.rs` + `ocs_launcher`) is restored and should stay.

---

## 9. Where the code lives

```
crates/elec/src/            domain: catalogue, component, expr, geometry, junction,
                            netlist, numbering, report, sides, terminal, xref (+ tests)
src/app/commands/elec.rs    every ECAD command, library statics, project handling
src/modules/electrical/     ribbon tab, PlaceSymbolCommand, WireCommand
src/ui/window/elec_library.rs   palette modal with thumbnails
src/app/commands/mod.rs     command registry (add new names here)
src/app/command_driver/utilities.rs  argument-taking commands must be listed here
```

Gotchas that cost time already:

- Commands that take a path must be registered in the **early-argument list** in
  `command_driver/utilities.rs`, or they run twice / execute the argument as a command.
- Block contents are owned by `record.handle` (or `record.entity_handles`) — **not**
  `block_entity_handle`; model space is `doc.header.model_space_block_handle`.
- Electrical collectors must skip entities whose `owner_handle != model_space`, or block
  geometry is counted as wires.
- Label height comes from the library entry (`label_height`), not a constant.

---

## 10. Suggested next steps

1. Finish or revert the uncommitted `ELECSHEET` fit; verify with
   `sheet_batch.txt` on `tests\proj2\sheet-01.dxf` and a screenshot.
2. Project-wide BOM that walks `project_sheets()` (the per-drawing `ELECBOM` does not yet).
3. Footprints for `ELECPANEL` (sizes per part) + DIN rail.
4. Wire numbering formats per sheet/project.
5. Re-run `cargo test --release` for the app and record the result in
   `reports\ecad-verification.md`.

---

## 11. Handy one-liners

```powershell
# build
cargo build --release -j 4

# version + graphics backend
.\target\release\OpenCADStudio.exe --version

# full headless draft of a test drawing
.\target\release\OpenCADStudio.exe --each tests\e2e_batch.txt tests\e2e_test.dxf

# electrical crate tests
cargo test --release -p elec
```
