# Manga Cleaner: what the app does

Manga Cleaner is a desktop app that removes Japanese text from manga and webtoon pages. It runs entirely on your
machine: no account, no upload, no Python. It finds speech bubbles and free text, keeps only the Japanese, fits a mask
at the page's own resolution, and cleans each region with the lightest engine that can do the job. Pixels outside an
edited region are unchanged, and colour mode, bit depth, embedded colour profile and metadata are carried through to
export.

## Home and projects

The app opens on a library. A **project** points at a folder of scans and holds one or more **chapters**; a chapter is
the thing you open and edit. **New project** asks for the source folder, a name, and the page mode (single page or
longstrip). The mode is permanent: it decides how pages are stitched and split in every chapter of the project, and
the dialog says so before you press Create. **New chapter** asks for the project, a title, a chapter number and a
source folder. The number starts at one past the highest the project holds and is yours to change; it is stored
exactly as you typed it and never renumbered, so a gap stays a gap, and Create is blocked on a number the project
already uses. Leave the folder empty and the chapter reads a subfolder named after it, or the project's own folder.
Only one chapter may read the project's own folder; a second attempt is refused, and the refusal names the chapter
that already has it.

Creating a chapter **copies its pages into the library**. A PNG or TIFF is copied byte for byte, and a JPEG, WebP,
GIF or BMP is decoded once and written as a lossless PNG, and that file is the page from then on. The scan folder is
read at that moment and never written to or read again, so you may move or delete it and the chapter still opens,
cleans, resumes and exports. The New chapter dialog says so, because a folder you were not told you could delete is
one you will keep. The cost is a second copy on disk, on the order of a gigabyte for a 200-page colour chapter, and
nothing checks the free space first: a page whose copy cannot be written is listed with the other skipped files
rather than failing the chapter.

Also on the library screen: **Open project** (with a filter field), **Rename** (the library's label only, the folder
on disk keeps its name), **Remove** (out of the library, pages on disk untouched), **Delete chapter**, **Copy source
path**, **Continue cleaning** on a chapter interrupted mid-run, and **Settings**, which includes the About section
with the version, licence, source offer, model versions and cloud terms. Deleting a chapter always removes its row,
its job file and the library's own copies of its pages; a tick in the same dialog also deletes the folder of scans it
was read from, except the project's own folder, which is refused so that deleting one chapter cannot empty the
project. Nothing goes to the system trash and undo does not reach across it, so the dialog names the folder and the
safe button is the primary one.

Each card shows the chapter and page counts, the mode, pages
cleaned out of the total, regions needing review, files skipped, and where a run stopped. Files are checked as a
chapter is read. A file whose header will not parse, that does not decode completely, that is not an image this build
reads, or whose colour mode has no policy here is **skipped and listed** with its reason, never cleaned halfway.
Dotfiles, `__MACOSX`, `Thumbs.db` and `desktop.ini` are skipped as junk. Files with identical contents are
deduplicated, on the converted file, so the same JPEG twice is one page; `001.jpg` beside `001.png` produces a warning
rather than a guess, and two sources that would convert onto one name get a stepped name and the warning still fires.
The chapter reports how many files were converted. AVIF, HEIC and JPEG XL need decoders this build does not ship and
are refused with the rest. Archives (CBZ, CBR, ZIP, PDF) are not accepted; point a chapter at a folder of images.

## The editor

The page sits in the middle, with four floating windows you can move, resize, collapse and close: **Pages**,
**Layers** (the toggle calls it Layers & review), **Tool options** and **Help**. The top bar carries the way back to
the library, the project and chapter name (editable in place), the view controls, and a Settings menu that also holds
Export. **Hold O** to see the original scan under your edits, or **Shift+O** to pin it on; a wipe slider sweeps
between the two. **M** toggles the mask overlay, which outlines every region at once. With the overlay off, a region
is outlined only while you point at it, focus it or select it. The bottom bar carries zoom (fit, a percentage, and
1:1), undo and redo, previous and next page, and previous and next region needing review; pinch and modifier-scroll
zoom as well. Page order is right-to-left by default and is set per project. A card in the corner lists the models
currently holding memory, with a button per row to free one; anything still needed loads again by itself. Notices
about skipped files, cloud rejections and memory pressure stack in the bottom-left corner, never over the image. The
editor autosaves, and reopening a chapter restores the page, scroll position, zoom and the pending review set.

Everything the app draws **on** the page is sky blue: region outlines, the region and canvas focus rings, the brush
cursor's footprint, the clone source ring, and a shape or an AI brush trail while you draw it. Manga is mostly black
ink, so a near-black mark disappears exactly where the work is. The declined and needs-review outlines keep their own
colours, because those say which state a region is in rather than merely where it is, and the paint brush's own colour
is still yours to choose. Pressing a control in a floating window never moves the page underneath it either: the
editor and the windows clip rather than scroll, and a row is brought into view by moving the one box that genuinely
scrolls, so Retry, Delete and the engine picker leave a zoomed canvas exactly where you put it.

## Pages and auto clean

The **Pages** window is a text list, not thumbnails, and it doubles as the progress indicator: during a run the marks
tick over page by page, so there is no separate progress bar. Each row shows the page number (or the position, in
longstrip), its status, and how many of its regions are done.

| Mark | Meaning |
|---|---|
| `·` | Not cleaned yet |
| `●` | Cleaning now |
| `✓` | Cleaned |
| `✓ 2` | Cleaned, 2 regions need review |
| `!` | Skipped, with the reason on hover |

**Auto clean** is the whole automatic job in one button: it finds text, keeps what the script gate reads as Japanese,
fits masks, picks an engine per region, cleans, and composites. Its options are **Scope** (Page, the default, or
Project), **Speech bubble text** (the engine a region inside a balloon starts on, Fill by default), **Text outside
bubbles** (the engine an out-of-balloon region starts on, LaMa by default) and **Outside bubbles** (Hold for review,
the default, or Clean anyway). Both engine rows offer the three local engines; FLUX and the cloud are per region only.
They are a **starting point, not a ceiling**: if the quality check rejects what an engine produced, the run climbs to
the next engine and tries again. An automatic run never goes past LaMa, and it never sends anything to the cloud; the
heavier engines are reachable per region, where you choose them and wait for them. A region LaMa declines is left
exactly as it was and listed for review, which is what the top of the ladder means.

Text **outside** a speech bubble is left alone unless **Outside bubbles** is set to clean it. Held for review, the
default, sound effects and lettering over artwork are listed with a **Clean anyway** button, which is where the "Text
outside bubbles" engine takes effect one region at a time. Set the other way, every out-of-balloon region on the run
goes to the ladder on that engine's rung with **no script read at all**: sound effects, and Latin lettering a
localiser added, are painted over with everything else. That is the row's stated meaning, it is why it is off by
default, and a resumed run goes back to holding them rather than carrying the choice.

A run is cancelled from the same place it was started, and the pages already cleaned are kept. When it finishes you
get a count of pages and regions; a chapter with no Japanese text reports "No text found across N pages" rather than
looking like a failure. If the weights are not on the machine, Auto clean is disabled with a sentence naming Settings,
Models. If the machine runs short of memory mid-run, it says what it will do instead: the FLUX helper stops, then the
redraw model is unloaded and the regions that needed it are listed for review, then it works one page region at a
time.

## The engine ladder

Every clean is done by one of five engines, and the app uses the lightest one that does the job: a heavier model on a
flat balloon is slower and often worse, because it can invent marks where there was only paper.

| Engine | Good for | Notes |
|---|---|---|
| **Fill** | Text on flat or gently graded paper, the common case | Samples the paper around the mask and lays down the tone it measured. No model needed, always available. |
| **Denoise fill** | Tight masks on noisy or JPEG scans | Smooths the fill together with the grain around it, so no seam is left. |
| **LaMa** | Screentone, halftone, line art behind the text | The default redraw engine, and the top of an automatic run. About 0.6 s a region on a GPU, 1.5 s on the CPU. |
| **FLUX** | The rare region nothing else reconstructs | Optional helper app you install yourself. Gigabytes of memory, around 20 s a region. Per region only. |
| **Cloud** | Complex art on a weak machine | Off by default, opt in, your own paid Google key. Per region only. |

Only the engines actually installed appear in a picker. A missing one is left out rather than shown greyed, and the
way to add it is Settings, Models. When no engine's output passes the quality check, the region is **declined**: the
original text is left exactly as it was, a marker stays on the page, and the region is listed for review with the
reason. Some sources restrict the ladder, because a redraw engine's output cannot be represented in them. An
indexed-colour page gets Fill alone. A CMYK page gets Fill and Denoise fill, which copy measured source values and so
cannot change the ink separations. A one-bit page cannot hold an inpainted result at all.

## Manual tools

Six tools sit in a rail on the right, selected with the keys `1` to `6`. Each has a dropdown of parameters; the
dropdown does not have to be open for the tool to work. Masks you draw by hand and masks the automatic pass made are
identical in kind: same row, same actions, same provenance, same export treatment. A hand-drawn mask is
**authoritative**: no detector runs on your gesture and no mask snaps onto a nearby region, so what you painted is
what is cleaned. The ring of paper around it is still sampled, so a fill matches the local paper rather than
defaulting to white.

### Auto Clean

Options: scope, speech bubble engine, outside-bubble engine, and the run button, all described above. It acts on the
page rather than on a stroke, so there is nothing to draw.

### Brush

Options: size, hardness, spacing, and a mode of Add, Erase or Paint. In Paint mode a colour, opacity and flow appear;
in Add and Erase they do not. A stroke is the union of the round discs the brush sweeps along your path, and that
shape is what is cleaned, not the rectangle around it. Add and Erase shape a mask that an engine then fills; Paint
lays down flat colour. Erasing where there is no mask says so rather than doing nothing silently.

### Shapes

Options: shape (Rect, Ellipse, Lasso, Polygon), mode, and feather up to 20 px. Mode is either **Solid colour**, which
covers the shape in a colour you picked, or one of the four cleaning engines, never both; the colour and opacity rows
appear only while the mode is Solid colour. It starts on Fill, the engine that samples the paper and matches it.

All four shapes are drawn on the page with a live preview of the shape itself: two corners for the rectangle and the
ellipse, with Shift constraining them to a square and a circle; a freehand loop for the lasso; and click-click-close
for the polygon, which finishes on its first vertex, on a double click, or with Enter, and needs three vertices before
any of those will close it. Escape abandons a shape in progress. What is committed is the outline, not the box around
it.

### AI Mask Brush

Options: Clean with (the engine, Fill by default), and size. Paint over the text, and the engine you named cleans
exactly the shape you painted. It differs from the Brush in one way: the Brush commits a planar fill, and this runs
whichever engine the row names. The engine is named outright rather than as a starting point, so a hand that picks
LaMa gets LaMa. The list is the same four engines a Layers row offers, under the same names.

### Content-Aware Fill

Options: fill mode (Match surround, Reconstruct or Solid), and engine (Local or Cloud). This one fills a mask that
already exists, so it is a click on a region rather than a stroke. Match surround is the planar fill from the ring of
paper, exact on flat and gently graded paper; Reconstruct is the redraw engine, for screentone and art crossing the
mask. This is the one tool that can reach the cloud, and it carries the whole confirmation flow for it.

### Clone / Heal

Options: size, hardness, opacity, flow, alignment (Aligned or Non-aligned) and mode (Clone or Heal). It starts on
Heal, aligned. Hold the source modifier and click somewhere on the page to set the copy source, then paint. The
modifier is Option by default and is a setting, in Settings, Shortcuts, because Alt is not a key every keyboard
prints. Clone copies the sampled pixels;
Heal blends the sampled texture into the destination's own tone. Escape clears the source. Painting before a source is
set tells you so.

## Layers and history

The **Layers** window lists every region on the current page, newest first. Collapsed, a row names the engine that
produced it and carries a delete control. Expanded, it shows provenance: engine, model version, fill mode, elapsed
time, whether it was made by hand or automatically, whether the automatic pass detected it, and for a cloud region the
cost and request id.

Each row can be re-run: **Try again** with the same setting, **Clean with** a different engine, or reopened in the
tool that made it with the mask intact. **Delete** removes the mask, brings the original text back, and takes the row
off the list, leaving no empty placeholder. **Dismiss** takes a flagged region off the list and leaves the page as it
is, and **Show on page** scrolls to a region and selects it. Everything here is undoable, and a cloud region is the
one thing a row cannot re-run. Clicking a region on the page selects it under every tool, lights its outline and
scrolls the matching row into view. A right click, or Shift+F10 from the keyboard, selects the region and offers the
same three things the row does.

**Needs review** is a filter at the top of the panel with a count, and it is the review surface. A region is flagged
when fitting failed and a model reconstructed the area; when the region is unusually large for the page; when every
engine failed the quality check, so the text was left alone; when the script gate skipped it, because it read as not
Japanese or because the gate was not confident enough (with the optional Japanese text reader installed, a balloon the
gate could not read is read once more before it lands here); when the text is outside a speech bubble; when a cloud
request was rejected, for any of five separate causes; or when a cloud request was **accepted**, unconditionally,
because a remote model can return something plausible and wrong. Gate-skipped regions carry a **Clean anyway** action.
Declined regions carry a marker on the page itself, since they are the one case where nothing visibly happened. The
bottom bar's up and down arrows step through the filtered set without entering the panel. Undo and redo are global
across every tool and are **written to the project folder**, capped at 500 entries, so a chapter reopened tomorrow can
still take back what was done today. Their tooltips name the edit they will reverse.

## Long-strip mode

A project created in longstrip mode treats a chapter as one continuous strip rather than a stack of separate pages.

- Nothing concatenates pixels, at any point, including export. The strip is a coordinate system laid over the source
  files.
- A bubble that crosses a join between two files is read and cleaned as **one region**. At export the patch is split
  at the page boundary, and both halves share one fill tone, because it is the same tone written twice.
- The strip is split into working segments at rows with the least detail, never through detected text, preferring a
  page join where one is close enough. Where no good row exists the split falls back to a fixed height and says so.
- The Pages window lists **positions** rather than files, and choosing one scrolls the strip to it. Previous and next
  page move by viewport, and the Layers panel lists what is on screen rather than the whole chapter.
- Duplicated overlap rows and misregistered joins between consecutive files are reported rather than silently
  stitched, and an unverified join is treated as a page edge.
- Memory is bounded: previews cap the short edge and tile the long axis, the scroller keeps about three screens, and
  only three pages' region state is held at once. A chapter of any length opens the same way.

## Export

Export is on the Settings menu in the top bar, and on **E**.

**Formats.** PNG (the default), TIFF (also the right choice for a stitched longstrip, and the only one that carries
CMYK), **PSD**, and CBZ, which is a zip of the per-page files stored rather than deflated. JPEG and WebP are refused
rather than approximated, and the refusal says what would work instead. PSB is not written.

**Layout.** One file per page, or one file for the chapter. Stitching a paginated project is refused, as is stitching
into a CBZ or into a PSD, and so is a stitched export whose pages disagree about colour mode, bit depth or colour
profile, or that uses indexed colour: one file has one of each of those. Each refusal is decided before a byte is
written.

**Masks.** Flattened is the image alone. Asked for separately, a PNG or TIFF export writes **a mask file beside each
page**, `001.png` and `001_mask.png`, an 8-bit grayscale PNG that is white where the lettering the page's edits
removed was and black everywhere else. It is the lettering, not the area an engine painted: a file made of the applied
masks would be a page of white blobs where the balloons are, which is not what someone asking "where was the text"
wants. For a PSD the same choice is the layered document instead, the untouched page as the Background and one masked
layer per region in a Cleaned group, and a layer's own mask stays the area it contributes. A CBZ refuses separate
masks, because a reader shows every file in the archive as a page.

**PSD.** Per page, in the source's own mode and depth: grayscale, gray with alpha, RGB, RGBA and CMYK at 8 and
16 bits, with the embedded profile carried through. Flattened, it is one Background layer holding the cleaned page.
Either way the merged image the file carries is the composite, so a reader that ignores layers still sees the cleaned
page, and a hidden region is a hidden layer that is still in the file. An indexed or sub-8-bit page, and a page over
30 000 px on a side, are refused for PSD rather than converted, decided from the chapter's own record before a folder
is made; the refusal names PNG, which carries them as they are.

**Destination.** A new sibling folder (the default), the source folder, or a folder you type or choose as a full path.
The source folder is never written to: choosing it gets you a refusal, and the dialog says so under the row rather
than letting you find out by pressing Export.

**Before you press Export** the dialog states two things: how many regions are still flagged for review (they export
as they are), and, for a stitched layout, how many pixels of paper white will be written beside the narrower pages,
since no source file has a pixel there. The finished notice reports what was actually written.

**What is preserved.** Outside the union of the applied masks grown by a small margin, the exported file's decoded
pixels are identical to the source's, and the colour mode, bit depth and embedded colour profile are unchanged. Colour
mode is never upgraded: an 8-bit grayscale source exports as 8-bit grayscale, whatever engine ran. Alpha is copied
through untouched, and 8 and 16-bit, Grayscale, RGB, CMYK and Indexed all survive. A page with no edits on it is
copied byte for byte and verified against the checksum taken when it was read, inside a CBZ too. Where a chosen format
cannot carry the source's colour mode, the app says exactly what will change before it runs, and never downgrades
silently.

## Settings

Settings is on **,** from anywhere. Changes apply at once and are remembered between sessions. It is five tabs,
General, Models, Acceleration, Shortcuts and About, and it opens on General, so the preference rows are the first
thing on screen with nothing to press. Every tab is one fixed-height scroller, so the dialog is the same size and the
Done button sits in the same place whichever tab you are on. General rows: **Theme**
(light, dark or system), **Reading direction** (the default for new projects; an open project keeps the direction it
was made with), **Cloud engines** (below), **Original view** (whether O shows the original only while held or toggles
it on), and **Language** (English is the only catalogue that ships today, though every string in the app, errors
included, is in it).

### Models

The engines and the runtime are downloaded after install rather than bundled, which keeps the installer small. This
section is a list of files: one row per model plus one for the runtime, each saying what it is, how large it is and
whether it is here, with **Download**, **Cancel**, **Check**, **Delete** and, when a stopped transfer left bytes
behind, **Discard partial**. A row with a remainder says how much is already here and that the next press continues
from there. Downloads are verified against a published checksum, and Check re-reads a file you suspect.

One row is optional in the strongest sense. The **Japanese text reader** (manga-ocr, three files, about 460 MB) is
offered here and nowhere else: nothing depends on it, it is not in the first-launch offer, and a machine without it
reads scripts exactly as this app did before the reader existed. Installed, it re-reads a balloon the language checker
could not name a script for, and a reading that is at least two characters and at least 60 per cent Japanese lets the
region clean instead of going to review.

On a first launch, if anything Auto clean needs is missing, the app offers the whole set in one dialog: the required
group (the text finder, the speech bubble finder, the language checker and its labels, plus the runtime) with no
ticks, and the redraw engine, manga-LaMa, as a tick that starts on. One press downloads what is ticked, runtime
first, one at a time. Where the redraw engine is already installed the choice section is not drawn at all, since there
would be nothing in it. The offer is made once; Settings, Models is the way back for anything declined. A weight the app
did not download (found beside the executable, in the bundle, or in a folder you set) reads as installed elsewhere and
has no Delete button. For an offline install, put the verified files in one of those folders by hand and the rows read
Installed. On Windows and on Linux x64, where more than one runtime build is published, the runtime row carries a
picker: the default works with every graphics card, and the CUDA builds need CUDA and cuDNN installed by you. Changing
the picker downloads nothing on its own, and the row says which build is actually installed when that differs from
the one chosen.

An optional **Hugging Face token** field sits at the bottom, because some downloads come from huggingface.co, which
limits anonymous transfers. It is kept in the operating system's credential store, sent to huggingface.co and no other
host, and never shown again once saved: the field is empty on every open and a line beside it says whether one is
stored. Where there is no usable credential store, or the keychain is locked, the app says which of the two it is and
keeps the token in the settings file as plain text rather than failing quietly.

### Acceleration

A picker of Automatic plus every device the installed runtime reports, and under it a read-only line per model saying
where it will run and whether that was measured here or guessed. A device that cannot be used is listed and disabled,
carrying its own reason (not available in this runtime, would split the model and run it slower, needs CUDA and cuDNN
installed, or this machine has less memory than that device needs). Automatic picks the fastest device known to work
for each model, which is not always the same one. A change takes effect on the next run; anything already loaded keeps
the device it started on.

Every Windows and Linux x64 download brings the **WebGPU plugin** beside the runtime, which is the cross-vendor GPU
path that carries the operator the redraw model needs. On Windows it sits next to DirectML, and a machine on a CUDA
build runs the detector on CUDA and the redraw model on the plugin rather than falling back to the processor. On Linux
x64 it is the whole GPU path and it needs the system Vulkan loader; without one the device is listed and disabled with
that as its reason. Linux aarch64 has no GPU package published for it and stays on the processor. None of those
placements has been run on real hardware, so each is reported as a guess rather than a measurement, and the memory
check, which needs a measurement to act on, never declines one.

### Shortcuts

The shortcut sheet, which is also what **?** opens, is where a shortcut is changed. Every chord in it is a button:
click one and the row listens, the next combination you press becomes the binding, Escape cancels, and Backspace or
Delete clears it (the row then reads Unbound and the command keeps whatever pointer route it has). A combination
another shortcut already answers is refused, not swapped, and the refusal names the shortcut holding it. Alt
combinations belong to the operating system and are not intercepted. A rebound row grows a Reset control, and the foot
of the sheet carries Reset all to defaults. The sheet also carries a **Pointer** group, which holds Clone / heal's
source modifier: Option, Command, Control or Shift, drawn as the keycaps of the machine you are on, because Alt is not
a key every keyboard prints. Escape is the one binding that cannot be changed. Only your differences
from the defaults are stored, so a binding survives a build that renames a default chord.

### Cloud

Cloud engines are **blocked by default**. While blocked, nothing is sent to a provider: the request is refused before
it is made, and the notice says so outright. Allowing it makes the cloud engine available to Content-aware fill, the
only tool that can reach it; an automatic run stays local whatever this is set to.

Before the first send of a session the app states what is transmitted: a bounded crop of the page, including the image
content around the text and not just the masked region, goes to Google, is processed there, and comes back. Nothing
else about the page or the project is sent. You bring your own key, stored in the operating system's keychain, never
in project files and never logged; a free-tier key is refused with the reason stated, because the unpaid tier trains
on submissions. The estimated cost is shown and confirmed before the first spend of a session unconditionally, and the
**Confirm before spending** setting governs the spends after that. A rejected request is not billed and falls back to
the local redraw engine.

### The optional redraw helper

Three rows appear for the optional FLUX helper app: the folder it is installed in, which runtime to load it through
(Automatic, MLX on Apple hardware, or SDNQ on any GPU), and which of the models in its weights folder to use. Only
models actually present are offered. A machine without the memory, without a GPU, or without the chosen runtime's
packages is told which of those it is.

## Updates

The app checks for updates on launch and from a **Check for updates** button in Settings. When one is available you
get the version and its release notes, and the choice of **Download & install** or **Later**; the download reports its
percentage and finishes with **Restart Manga Cleaner**. Updates are downloaded from cryptographically signed
manifests. An update never silently changes a model or a default under a project you already have: a re-run after a
change is recorded as a new provenance entry beside the old one. There is no background telemetry. Statistics are
written into the project file and readable in review; crash reports are written locally and surfaced on the next
launch, never sent anywhere without an explicit action.

## Keyboard shortcuts

Every tool and every bottom-bar action has a key, the editor is fully operable without a pointer, and every binding
but Escape can be changed or cleared in Settings, Shortcuts. Tool, view, zoom, navigation and panel keys work while a
chapter is open.

| Key | Does |
|---|---|
| `1` | Auto clean |
| `2` | Brush |
| `3` | Shapes |
| `4` | AI mask brush |
| `5` | Content-aware fill |
| `6` | Clone / heal |
| `O` (held) | Show the original while held |
| `Shift`+`O` | Pin the original on |
| `M` | Mask overlay |
| `R` | Show only the regions that need review |
| `U` | Undo |
| `Shift`+`U` | Redo |
| `Cmd`/`Ctrl`+`Backspace` | Delete the selected layer |
| `0` | Fit the page |
| `+` (or `=`) | Zoom in |
| `-` | Zoom out |
| `Z` | Actual size, 1:1 |
| `←` / `[` | Page left |
| `→` / `]` | Page right |
| `N` | Next region needing review |
| `P` | Previous region needing review |
| `F` | Pages window |
| `L` | Layers & review window |
| `T` | Tool options window |
| `F1` | Help window |
| `E` | Export |
| `N` (library) | New project |
| `Cmd`/`Ctrl`+`O` (library) | Open project |
| `,` | Settings |
| `?` | This shortcut list |
| `H` | Back to the library |
| `Escape` | Cancel or close |

Arrow keys follow the reading direction: in right-to-left, the default, `←` is next. `N` is New project on the library
screen and next-review in the editor, which is why the two never collide. A bare `Delete` or `Backspace` still deletes
from a focused Layers row.
