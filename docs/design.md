# Hydra design system

How Hydra looks, and why. Every tab is built from what is here, so a new feature looks like the rest of the app without a design pass of its own.

- **In code:** `crates/hydra/src/theme.rs` holds the colours, fonts, type scale and spacing, and `theme::apply` installs them at start-up. Tabs take colours from `theme::LIGHT` and text styles from egui's `TextStyle`s or `theme::heading()`, `label()` and `overline()`. They never write a literal `Color32` or font size.
- **The boards:** the design canvas at <https://claude.ai/artifact/2kf45qw3E7AqKkBPAcYSzZ> (private to its owner) shows the foundations, the components, and the Schematic and Proposals tabs redrawn in the system. Its source is kept in [`docs/design/`](design/), so the boards can be rebuilt if the canvas is ever lost. Each `.dc.html` file is one board, and `canvas.json` lays them out. The files open in the claude.ai Design canvas, not in a plain browser.

## Who it is for

The users are hydraulic engineers who design manifold blocks and are used to paper datasheets and hand-drawn circuits. So the app should feel like a drawing office, not a web app:
- **The schematic is a sheet of paper.** It is white with a 5 mm grid, sits on a grey desk, and has a title block in its corner, like the paper drawings.
- **The app around the sheet is quiet.** Grey chrome, black text, sturdy edges.
- **Colour means something.** Blue is what you are acting on. Amber, red and violet are valve status. Green is a finished human decision. Teal is the AI assistant's work. Nothing is coloured for decoration.
- **Sturdy, not delicate.** 14 px body text, 1 px borders dark enough to see at a glance, 3 px corners, 28–36 px controls, no hairlines, no pill shapes, and controls that don't grow under the pointer.

## Colour

Light is the only theme. Token names match the fields of `theme::Palette`.

| Token | Light | Use |
| --- | --- | --- |
| `chrome` | `#E9ECEF` | App frame, tab strip, toolbars |
| `panel` | `#F4F5F7` | Side panels, inspector, issue list |
| `surface` | `#FFFFFF` | Inputs, cards, tables, dialogs |
| `hover` | `#EEF1F4` | Behind a hovered control |
| `sheet` | `#FFFFFF` | The drawing paper |
| `grid`, `grid_major` | `#E4E8ED`, `#CFD5DC` | Sheet grid lines every 5 mm and every 25 mm |
| `desk` | `#CDD3DA` | Around the sheet |
| `ink` | `#14181F` | Text and drawing lines |
| `secondary` | `#4A5361` | Labels, meta, hints |
| `border` | `#C3C9D1` | Card edges and panel dividers |
| `divider` | `#DDE1E6` | Lines between rows of a list or table |
| `border_strong` | `#7F8895` | Edges of inputs and buttons |
| `accent` | `#1F5FAD` | Primary buttons, selection, focus, links, the active tab. Never a status |
| `accent_hover`, `accent_tint`, `accent_text`, `on_accent` | `#174C8C`, `#E3ECF7`, `#174C8C`, `#FFFFFF` | Hovered primary button; selected row; text on the tint; text on the accent |

Status colours come as a `theme::Tone`. Each has three parts: `strong` for an edge or a canvas outline, `tint` for a background, and `text` for words on that tint or on a panel.

| Tone | Strong / tint / text | Means |
| --- | --- | --- |
| `unlisted` | `#C07A00` / `#FFF1D6` / `#8A5300` | Part not listed: the valve type is in the catalogue but this part number is not. Also warnings and unchecked imports. |
| `missing` | `#C4291C` / `#FDE7E5` / `#A3201A` | Missing from the catalogue. Also errors that block a save or an approval. |
| `stand_in` | `#6A3FC9` / `#EEE8FB` / `#5B33B8` | Drawn with another valve in place of the one asked for. |
| `ok` | `#2A8A52` / `#E2F2E8` / `#1E6B3E` | Finished human decisions only: an approved proposal, a checked revision. A normal valve shows no colour at all. |
| `agent` | `#0E7C86` / `#E0F2F3` / `#0B5F67` | Work done by the AI assistant: drafts, imports, its activity. |

Rules:
- **Status is never colour alone.** It always comes with a word and a mark of its own shape: ▲ part not listed or warning, ■ missing or error, ⇄ stand-in, ✓ approved or checked, ◆ assistant, ● waiting, ✕ rejected. That way a status survives a black-and-white printout and reads for colour-blind engineers.
  - The marks are painted as shapes, not typed. Plex has none of ▲ ■ ◆ ● ✕. egui's fallback fonts cover some of them in some families, and ✕ in none.
- **Contrast is checked.** Text is at least 4.5:1 on every background it sits on. Edges and marks that must be seen (input borders, status frames, the accent) are at least 3:1. Amber and red also differ in lightness, not hue alone. The theme tests check all three for every palette.
- **The sheet keeps ISO 1219 as on paper.** Lines are black ink and line kinds differ by dash pattern. UI colour reaches the sheet only as an overlay: the selection frame (2 px accent) and the status frame (`strong`). It never colours the lines themselves.

| Line | Weight | Pattern |
| --- | --- | --- |
| Working | 0.50 mm | solid |
| Pilot | 0.35 mm | dash |
| Drain | 0.35 mm | short dash |
| Block outline | 0.35 mm | dash-dot |

## Type

IBM Plex, chosen to match the lettering of the engineers' paper drawings. It is bundled in the binary from the unmodified upstream files in `crates/hydra/fonts/`, under the SIL Open Font License. The licence is in `OFL.txt` and can be read in the app (Account tab, "Font licence: IBM Plex"). egui's own fonts stay behind Plex as fallbacks.

| Style | In code | Font | Use |
| --- | --- | --- | --- |
| Title | `TextStyle::Heading` | Plex Sans SemiBold 20 | A tab's or a proposal's main heading |
| Heading | `theme::heading()` | Plex Sans SemiBold 16 | Section headings in a panel or a detail view |
| Body | `TextStyle::Body` | Plex Sans 14 | Everything else |
| Label, button | `theme::label()`, `TextStyle::Button` | Plex Sans Medium 13 | Field labels, button text |
| Caption | `TextStyle::Small` | Plex Sans 12 | Meta: who, when, from where |
| Overline | `theme::overline()` | Plex Sans Condensed SemiBold 12, capitals | Panel headers and title-block labels: DRAWN BY, PARTS |
| Data | `TextStyle::Monospace` | Plex Mono Medium 13 | Codes, `key@version` refs, part numbers, ports, values with units |

- Nothing in the UI is smaller than 12 px.
- Designators (V1, P, T) use the `theme::MONO_SEMIBOLD` family.
- Overlines are written in capitals by the caller. egui has no text-transform.

## Spacing and shape

- **Spacing:** a 4 px grid, using 4 / 8 / 12 / 16 / 24 / 32. Panels have 16 px padding and dialogs 16–32.
- **Control heights:** 28 px for toolbars and inline controls, 32 px for form fields and dialog buttons, 36 px for list and issue rows.
- **Corners:** `theme::RADIUS` (3 px) on controls, cards and chips. Panels, the sheet, title blocks and tables are square.
- **Edges:** `border` for panel edges, `divider` between rows, and `border_strong` for anything you can click or type in. Focus and selection are 2 px `accent`.
- **Shadows:** only floating menus and dialogs have one.
- **Icons:** 16 px, 1.75 px stroke, painted. No emoji.

## Components

The Components board shows each one. How they map to egui:

- **Buttons:**
  - *Primary* is filled `accent` with `on_accent` text, and there is at most one per view: the action that commits.
  - *Secondary* is the egui default button.
  - *Quiet* is a frameless button in `accent`, for toggles and links inside panels.
  - *Danger* is outlined in `missing.text`, never filled, and is used for reject, delete and discard.
  - A disabled button always says why in its tooltip (`on_disabled_hover_text`).
- **Toolbar groups:** buttons grouped by job with joined edges (file, edit, place, export). A toggle shows its key (`W`) and turns `accent_tint` while on.
- **Tabs:** the active tab has a 3 px `accent` top edge and the panel colour. Proposals carries a count of waiting proposals in the `agent` tone.
- **Fields:** a label above, 32 px high.
  - Codes and refs are in Data type.
  - A changed field gets an `unlisted` edge, a CHANGED tag and "Was …" under it.
  - A field with an error gets a `missing` edge and the fix in plain words under it.
- **Status chips:** the mark, then the word, in the tone's text on its tint with a `strong` edge, 24 px high. Valve status is about the catalogue. Review status (waiting, approved, rejected, checked) is about a person's decision.
- **Banners:** a full-width strip in a tone's tint with a `strong` edge: the mark, a bold first sentence, the fix, and at most one button. An unchecked import, for example, offers "Mark checked".
- **Issue list:** one row per issue: the severity word with its mark, the part as a link that selects it on the sheet, then the message. Errors come first.
- **Title block:** in the corner of the sheet: the circuit name, schema, drawn by, revision with an "edited" mark, and checked by. It replaces the toolbar's revision text and prints in the SVG and PDF.
- **Dialogs:** a 16 px title, a sentence saying that nothing has been lost and what the choice is, then the buttons right-aligned on a panel-coloured footer, with the primary button last.

## Layout of a tab

The Schematic and Proposals boards set the pattern:
- an app bar with the tabs and, on the right, the database and the signed-in user;
- a toolbar;
- banners;
- then side panels (lists on the left, the inspector for the selection on the right) around the main view;
- issues docked at the bottom;
- a status bar with coordinates, zoom and the keyboard hints.

The Proposals tab adds three things:
- a review queue on the left;
- a detail view that shows what the assistant did, step by step with the datasheet page for each step, puts the symbol beside the datasheet page it came from, and shows the fields as a table with their source page;
- an assistant activity feed on the right.

Approve stays off while a problem is open, and the action bar says why.

## Not built yet

The boards show the target. `theme.rs` is in place, but these parts are still to come in the restyle PRs (see TODO.md):
- **The sheet:**
  - the grid and the desk around it (the canvas is plain white today);
  - the title block;
  - line weights: one stroke weight is drawn today. Pilot and drain share one dash, and the drain's short dash isn't drawn yet;
  - the 2 px selection frame (1.5 px today);
  - lettering in Plex in the SVG and PDF exports (they use Ubuntu Light).
- **The tabs:**
  - the tab strip's 3 px accent edge and the Proposals count;
  - toolbar groups and primary buttons;
  - painted status marks;
  - status chips, banners and the issue list;
  - the side panels and the status bar.
- **Older literals:** a few places in older code still write literal colours or sizes: `schematic.rs` (sheet white and black ink), `viewer3d.rs`, and the catalogue's drop overlay. They move to the palette as their tab is restyled.

## Adding a theme

1. Add a `Palette` constant beside `LIGHT` in `theme.rs`, with every token chosen for the new ground.
2. Add it to `PALETTES` in the theme tests, so its contrast is checked.
3. Pass it to `theme::apply`. `apply` always selects egui's light theme today (`set_theme(Theme::Light)` and the style set on `Theme::Light`). A dark palette also needs `apply` to select `Theme::Dark` and set its style there, or egui will report `dark_mode` as false and pick light defaults.
4. Tabs read `theme::LIGHT` directly today. With a second theme, `theme` gains a function returning the palette in use (kept in egui's memory by `apply`), and each `theme::LIGHT` becomes a call to it: a mechanical replace, because no tab writes a literal colour.
5. Add its column to the colour tables above.
