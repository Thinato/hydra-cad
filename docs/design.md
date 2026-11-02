# Hydra design system

How Hydra looks, and why. Every tab is built from what is here, so a new feature looks like the rest of the app without a design pass of its own.

- **In code:** `crates/hydra/src/theme.rs` holds the colours, fonts, type scale and spacing, and `theme::apply` installs them at start-up. Tabs take colours from `theme::LIGHT` and text styles from egui's `TextStyle`s or `theme::heading()`, `label()` and `overline()`. No tab writes a literal `Color32` or font size; the 3D view's ground and its models are palette tokens too. Kittests build their harness with `theme::harness(size, app, state)`, which installs the theme and skips kittest's first frame, drawn before the theme exists. Older tests wrap a harness in `theme::themed`.
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
| `viewport` | `#2B3139` | The 3D view's ground, dark so the shaded grey models stand out. No text is drawn on it, so it has no contrast pair |
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
  - The marks are painted as shapes, not typed: `theme::Mark` names them and `Mark::paint` draws one in a rect. Plex has none of ▲ ■ ◆ ● ✕. egui's fallback fonts cover some of them in some families, and ✕ in none.
- **Contrast is checked.** Text is at least 4.5:1 on every background it sits on. Edges and marks that must be seen (input borders, status frames, the accent) are at least 3:1. Amber and red also differ in lightness, not hue alone. The theme tests check all three for every palette.
- **The sheet keeps ISO 1219 as on paper.** Lines are black ink and line kinds differ by weight and dash pattern. UI colour reaches the sheet only as an overlay: the selection frame (2 px accent) and the status frame (2 px `strong`), both with `RADIUS` corners. It never colours the lines themselves. The one exception is the title block's "edited", "not saved" and "not yet" words, lettered in `unlisted.text` on the canvas.
- **One drawing everywhere.** Each line carries its `hydra_symbols::Pen`, which says its weight and dash, so the canvas, the SVG and the PDF draw it alike. The canvas never draws a line thinner than 1 px. Exports are pure black on white, as printed and photocopied; the canvas uses `ink`.

| Line | `Pen` | Weight | Pattern |
| --- | --- | --- | --- |
| Working | `Working` | 0.50 mm | solid |
| Pilot | `Pilot` | 0.35 mm | dash 1.6, gap 0.8 |
| Drain | `Drain` | 0.35 mm | short dash 0.6, gap 0.6 |
| Block outline | `Outline` | 0.35 mm | dash-dot, drawn as separate dashes |
| Symbol bodies, port circles | `Body` | 0.35 mm | solid |
| Status frame | `Frame` | 0.25 mm | dash 1.0, gap 0.8 |
| Title block frame / rules | `Border` / `Rule` | 0.50 / 0.25 mm | solid |

Symbol bodies are 0.35 mm, not 0.50: a 10 mm envelope's springs, arrow heads and spool lines fill in at 0.50, and the lighter body lets the working lines read as the circuit's paths.

- **The sheet** is the smallest landscape ISO A size, A4 to A0, that holds the drawing with 10 mm of paper round it and the title block under it (`hydra_symbols::sheet`). The drawing is centred above the title block, and the sheet's corner lies on the 5 mm grid, so the grid lines up with the drawing's coordinates; the sheet keeps 5 mm spare for that snap, so the margin is never under 10 mm. While a part is dragged, the sheet stays the document's. A drawing larger than A0 gets a sheet cut to fit it. The canvas paints `desk` round the sheet, a 1 px `border_strong` edge, and the grid: `grid` every 5 mm and `grid_major` on the drawing's multiples of 25 mm, the 5 mm lines left out once they would be closer than 4 px. Exports are the sheet, without the grid. Double-click fits the whole sheet.

## Type

IBM Plex, chosen to match the lettering of the engineers' paper drawings. It is bundled in the binary from the unmodified upstream files in `crates/hydra/fonts/`, under the SIL Open Font License. `hydra-symbols` includes Plex Sans Regular, SemiBold and Condensed SemiBold from the same folder as `PLEX_SANS`, `PLEX_SANS_SEMIBOLD` and `PLEX_SANS_CONDENSED_SEMIBOLD`, which `theme.rs` installs too, so the binary holds each face once; the PDF export embeds them, the SVG names the same families, so the exports letter as the canvas does. The licence is in `OFL.txt` and can be read in the app (Account tab, "Font licence: IBM Plex"). egui's own fonts stay behind Plex as fallbacks.

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
- **Edges:** `border` for panel edges, `divider` between rows, and `border_strong` for anything you can click or type in. Keyboard focus is a 2 px `accent` ring, and so is the selection frame on the sheet. A selected list row is quieter: `accent_tint` with a 1 px `accent` edge (see List rows).
- **Shadows:** only floating menus and dialogs have one.
- **Icons:** 16 px, 1.75 px stroke, painted. No emoji.

## Components

The Components board shows each one. How they map to egui, with the `theme` helper where there is one:

- **Buttons:**
  - *Primary* (`theme::primary(text)`) is filled `accent` with `on_accent` text, and there is at most one per view: the action that commits.
  - *Secondary* is the egui default button.
  - *Quiet* is a frameless button in `accent`, for toggles and links inside panels.
  - *Danger* (`theme::danger(text)`) is outlined in `missing.text`, never filled, and is used for reject, delete and discard.
  - Both helpers return an `impl Widget` around an egui `Button`, 32 px high, so `add_enabled` and the hover texts work as usual. Under the pointer or pressed, primary turns `accent_hover` and danger takes the `missing` tint and strong edge. With keyboard focus both get the 2 px accent ring. A disabled primary button is the accent faded by egui.
  - A disabled button always says why in its tooltip (`on_disabled_hover_text`).
- **Toolbar groups:** buttons grouped by job with joined edges (file, edit, place, export), built with `theme::group(ui, add)`: one `border_strong` edge, square controls inside that share their 1 px `border` edges as dividers. Each control fills itself, so a disabled one fades to the surface. A toggle is a `Button::selected` with its key as `right_text` (`Draw wires W`) and turns `accent_tint` while on. The Schematic toolbar's controls are 32 px, the primary Save button's height, so a group's joined edges line up.
- **Tabs:** egui_dock's strip, styled by `theme::dock_style(style)` in one place: square 32 px tabs on `chrome`, inactive ones in `secondary` with no edge, the active one `ink` on `panel` with a `border` edge, and keyboard focus as an `accent` edge. egui_dock can't draw the active tab's 3 px `accent` top edge, so `theme::active_tab_edge(painter, rect)` paints it over the tab once the dock is drawn, from the rect egui_dock reports in `TabViewer::on_tab_button`. Proposals carries the number of proposals waiting for review as a badge, `theme::counted(title, n)`: the count in Plex Mono SemiBold 12 in `on_accent` on the `agent` tone's strong colour, square, as chips are not pills. It counts the list the Proposals tab loaded, so it shows nothing before that list arrives, with none waiting, or signed out.
- **Fields:** a label above, 32 px high: `theme::field(ui, name, edit)` puts `name` in label type over the `TextEdit` and labels the edit with it. In a table (the Proposals fields) the label is the first column instead.
  - Codes and refs are in Data type.
  - A changed field gets an `unlisted` edge, a CHANGED tag and "Was …" under it. In a table, the row is tinted `unlisted`, the label gets a ▲ Changed chip, and "was …" sits in its own column.
  - A field with an error gets a `missing` edge and the fix in plain words under it.
- **Status chips:** `theme::chip(ui, mark, word)`: the mark, then the word, in the tone's text on its tint with a `strong` edge, 24 px high. Kittests and screen readers read the word as a label. The mark picks the tone (`Mark::tone`): ▲ `unlisted`, ■ `missing`, ⇄ `stand_in`, ✓ `ok`, ◆ `agent`, ● the accent (waiting for a person), ✕ grey (rejected). Valve status is about the catalogue. Review status (waiting, approved, rejected, checked) is about a person's decision.
- **Marked text:** `theme::marked(ui, mark, text)`: the mark, as tall as the text, then the text, both in the mark's tone and always left to right. Used for a problem line, a queue row's check or a "No symbol" note.
- **Banners:** `theme::banner(ui, mark, first, rest, action)`: a full-width strip in the mark's tint with a `strong` edge: the mark, centred on the first line, the bold first sentence, then the fix, and after it what `action` adds, so it is read in that order. The board allows at most one button: an unchecked import offers "Mark checked". Pass `|_| ()` for none.
- **Tables:** square, a `border` edge, `divider` lines between rows, a `panel`-coloured header row in Overline type and `secondary` colour, and 12 px cell padding. `theme::table(ui, width, rows)` draws the frame (`width` includes its edge, so `ui.available_width()` fits without growing a resizable panel or window), `theme::table_row(ui, fill, cells)` one row (`panel` fill for the header), `theme::cell(ui, width, add)` a column and `theme::column_title(ui, title)` a header. The Proposals fields and parts and the Catalogue's parts and editor use them. The Catalogue's valve-type list is a table whose body rows are `list_row`s, so they select like the review queue.
- **Symbol preview:** `schematic::symbol_preview(ui, size, symbol, caption, right)`: the symbol fitted on the `sheet` colour over a `panel` caption strip (`size` includes the edge, as with tables), the caption at its left and the symbol's reference (or ■/▲ No symbol) at its right. The Proposals detail and the Catalogue detail and editor show symbols this way.
- **Panel sections:** `theme::section(ui, add)`: a strip as wide as the side panel, 12 × 10 px padding over a `border` line. A panel's header (an overline title such as PARTS, and a count or buttons) and each section of the inspector (CATALOGUE, USE INSTEAD, PLACEMENT, titled in overline type in `secondary`) are sections.
- **List rows:** `theme::list_row(ui, selected, name, add)`: a 1 px `divider` under each row; the selected row is `accent_tint` with a 1 px `accent` edge all round (never a coloured stripe down one side), and a focused row gets the same edge. A clickable row tells screen readers its name and whether it is selected (`WidgetInfo::selected`). The review queue and the parts list use it.
- **Segmented control:** joined `Button::selected` segments in a `theme::group`; the selected one is `accent_tint`. The review queue's Waiting / All and the Issues / Extra BOM lines / Import notes switch under the sheet are ones.
- **Issue list:** one row per issue: the severity word with its mark, the part as a link that selects it on the sheet, then the message. Errors come first.
- **Title block:** in the sheet's bottom right corner, 10 mm in, drawn by `hydra_symbols::title_block` so it prints in the SVG and PDF too. A 120 mm box (wider for a long name), a 0.50 mm frame and 0.25 mm rules: the circuit name in Plex Sans SemiBold and "schema N" on the first row, then DRAWN BY, REVISION and CHECKED in columns two, one and one quarters wide, their labels in Plex Sans Condensed SemiBold capitals (`secondary` on the canvas). It says only what Hydra knows: an import was drawn by the AI assistant, and at its first revision it is checked "not yet"; anything else reads "—". Hydra records no check, so the block never says a circuit was checked. The revision reads "not saved" for a new circuit, and "4, edited" with unsaved changes, in an export too, since an export is the circuit as it is. It replaces the toolbar's revision text; screen readers and kittests read it from the sheet's accessible label.
- **Dialogs:** a 16 px title, a sentence saying that nothing has been lost and what the choice is, then the buttons right-aligned on a panel-coloured footer, with the primary button last. `theme::dialog(ctx, id, title, body, footer)` builds one as a modal, 420 px wide: the title in heading type, `body` in 24 × 20 px padding, then `footer`'s buttons on the `panel` footer under a `divider` line. The footer is laid out right to left, so `footer` adds the primary button first; what it returns comes back in the `ModalResponse`. The sign-in window and the Account tab's Set password and Delete windows use it.

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

Built so far: the queue, each waiting row with its check (■ problems, ▲ replaces a released type, ✓ checks pass), the header with its chips, the symbol on the sheet, the replace banner, the fields and parts tables, the notes, and the action bar pinned under the detail with its problem count. The step list, the datasheet page beside the symbol, each field's source page and the activity feed wait for data hydra-mcp doesn't record yet (see TODO.md).

The Schematic tab follows the board around the sheet:
- the toolbar's groups: file (Save as the primary button), edit, place (valve type, Place, Add port, Add part), the Draw wires toggle, and Export at the right, still read last; below them the circuit's name, the note for the next save and what happened last;
- the sheet on the desk, with its grid and title block;
- the unchecked-import banner with Mark checked;
- the parts list on the left: valves with their series and code and a status chip, items, and block ports as chips; a click selects the part;
- the inspector on the right: the part's designator and what it is, its status chip and why in plain words, CATALOGUE, USE INSTEAD with the stand-ins and the port mapping, the label or bill-of-materials fields, and PLACEMENT (Rotate R, Mirror M, Delete, In block, Plugged); for a selected wire its kind, Delete wire and Draw net automatically;
- the panel under the sheet: the issues, or the extra bill-of-materials lines or the import notes, chosen with a segmented switch;
- the status bar: the pointer's position on the sheet in mm, the zoom in points per mm (not a percentage, since screens differ in how big a point is) and the essential keys for the mode, with every key in its hover.
- An issue links the part it is about only when it knows the part's id (`Issue::component`, or the part the outline is cut round); the drawn-wire messages link none. Nothing is guessed from a message's words.

The Catalogue tab has no board of its own; it is built from the same parts:
- a toolbar: Search with its label, All versions, how many valve types are shown, and New valve type (secondary, disabled offline with the reason);
- the valve types as a table filling the tab, since engineers scan a catalogue like a paper one: SERIES, CODE, MANUFACTURER, MOUNTING, VERSION, STATUS and DATASHEET, codes in Data type, rows one line high (only those in view are laid out) and selectable like the review queue, named "series code, version N". STATUS says only what is wrong: ■ Symbol missing, ▲ Version N is newer, ▲ Draft, no symbol; a normal type says nothing;
- the selected type's detail as a side panel on the right, in sections like the inspector: its title, manufacturer and `key@version`, "A newer version exists", Place in schematic (the view's primary action, disabled with the reason when the latest version has no symbol) and Edit as new version; SYMBOL as a symbol preview with what is wrong with it; MOUNTING; PARAMETERS; PARTS as a table; DATASHEET;
- the new-version editor as a dialog: the fields as a table, the symbol it will draw, the error as a ■ line, then Cancel and Save as new version (primary, last);
- while a file is dragged over the window, a scrim of `ink` at 80 % covers it with the drop hint in Title type in `surface`.

The Account tab has no board either; it is a column of sections, each titled in overline type:
- the "Offline: no database." banner in the `unlisted` tone when there is no database, with how to set one up;
- ACCOUNT: "Signed in as …" in heading type, the server in Data type, whether the password is remembered, and Sign out; or "Connected as … through HYDRA_DATABASE_URL";
- YOUR PASSWORD: labelled 32 px fields, Change password (primary), and each problem as a ■ line in plain words;
- USERS, for administrators: Refresh, the users as a table (NAME, ROLE, SIGNING IN, SESSIONS and an Actions menu per row) where only what is wrong is marked (✕ Disabled), then the New user fields and Add user (primary). On your own row the actions the rules forbid are off and say why;
- ABOUT: the Hydra version in Data type and the font licence.

The 3D tab has no board; it is built from the same parts:
- a toolbar with Preview a model file… (secondary: nothing in the tab commits, so it has no primary button); while a file is previewed, Close preview, the file's name and its size in Data type;
- the side panel on the left, in sections: BLOCK with the block picker and its `key@version` in Data type; FACES as a table (FACE with the face name in Data type over its pattern, VALVE with a picker offering only valves whose mounting fits); SELECTED, for a valve selected in the schematic, with its designator, series and code, its ref and the face it is on; then what the view can't show as marked lines: ■ errors (a mount that doesn't fit, a valve missing from the catalogue, a model that failed to load), ▲ warnings (a valve not on the block, a part on the block but drawn outside it, a block or valve with no 3D model), and offline the `secondary` hint that models come from the shared database;
- the view on the `viewport` ground, the block shaded from `border`, valves from `border_strong` and the valve selected in the schematic from `accent`, as selection is everywhere; above it, in `secondary`, "Loading N model(s)…" or, when nothing is chosen and nothing is noted, how to see something;
- the status bar on `chrome` with how to move round the view.

The strip above the tabs shows only when there is something to show: a database error as a `missing` banner, its first sentence in bold; then the latest notice in `secondary` and the update link at the left, and at the right, as in the boards' app bar, the database's name in Data type (the server in its hover), "Signed in as …", Retry after a failed connection, and Sign out.

## Not built yet

The boards show the target. Every tab is on the theme, but these parts are still to come (see TODO.md):
- **The sheet:** the title block's date and who drew or checked it, which need the revision's author and time from the database (the editor doesn't get them today); designators in Plex Mono SemiBold on the sheet, as the boards letter them (the sheet letters everything in Plex Sans).
- **The tabs:**
  - the app bar itself: the tabs are egui_dock's strip, so the database and user sit in the strip above it rather than beside the tabs, and the database has no connection dot;
  - the active tab's title in SemiBold, and tab titles at 14 px: egui_dock tells `TabViewer::title` nothing of which tab is active, so every title is in button type (Plex Sans Medium 13);
  - icons on the Undo and Redo buttons, which are words today;
  - on the Proposals tab: the assistant's step list, the datasheet page beside the symbol, each field's source page and the activity feed, which need new data from hydra-mcp;
  - the Catalogue's new-version editor and the Schematic's dialogs are still egui windows with their buttons on the left, not `theme::dialog`.

## Adding a theme

1. Add a `Palette` constant beside `LIGHT` in `theme.rs`, with every token chosen for the new ground.
2. Add it to `PALETTES` in the theme tests, so its contrast is checked.
3. Pass it to `theme::apply`. `apply` always selects egui's light theme today (`set_theme(Theme::Light)` and the style set on `Theme::Light`). A dark palette also needs `apply` to select `Theme::Dark` and set its style there, or egui will report `dark_mode` as false and pick light defaults.
4. Tabs read `theme::LIGHT` directly today. With a second theme, `theme` gains a function returning the palette in use (kept in egui's memory by `apply`), and each `theme::LIGHT` becomes a call to it: a mechanical replace.
5. Add its column to the colour tables above.
