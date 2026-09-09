/**
 * The English catalogue.
 *
 * Nested by area, one nesting level per key segment, so a key like
 * `masks.value.cloudCost` is `en.masks.value.cloudCost`. `index.js` flattens it
 * once at module load; nothing walks this object at render time.
 *
 * Rules the copy follows, and a translator inherits:
 *
 * - **Named placeholders, never concatenation.** `{count}`, `{path}`, `{name}`.
 *   Word order is the translator's to change.
 * - **A `{…Key}` placeholder carries another key**, resolved before it is
 *   interpolated. `{statusKey}` is a key; `{status}` would be a literal.
 *   Task 3's notices depend on this and so does every provenance row.
 * - **A `{name:format}` placeholder is formatted, not stringified.** The only
 *   format today is `currency`, and `masks.value.cloudCost` is the only place
 *   money is written. Nothing else in the app may print a `$`.
 * - **A string may be an object of plural forms** - `{zero, one, other}`,
 *   selected against `params.count`. `zero` is not an English `Intl.PluralRules`
 *   category; `t()` honours it anyway, because "0 regions need review" is a bad
 *   sentence and the zero case is the common one.
 * - **Proper nouns are not translated.** Licence identifiers, URLs, provider
 *   and model names, format names (PNG, JPEG, CBZ) and version strings arrive
 *   from the backend as values, and are interpolated, not looked up.
 * - **Errors name the problem and the recovery.** A refusal says what it
 *   refused and what to do instead.
 * - No emoji. No string assembled from fragments in a component.
 */

export const en = {
  /* ================================================================== */
  /* app - the product's own name                                        */
  /* ================================================================== */
  app: {
    name: {
      mangaCleaner: 'Manga Cleaner',
    },
  },

  /* ================================================================== */
  /* shell - dialog and notice furniture that belongs to no one screen   */
  /* ================================================================== */
  shell: {
    action: {
      cancel: 'Cancel',
      close: 'Close',
      done: 'Done',
      continue: 'I understand, continue',
      confirmSpend: 'Send to the cloud',
      convert: 'Convert',
      dismissNotice: 'Dismiss',
      // Beside a path field, never instead of one: the field can also be typed
      // into, and outside the desktop window there is no chooser to open.
      chooseFolder: 'Choose folder…',
    },
  },

  /* ================================================================== */
  /* modal - dialog titles, and the bodies of the dialogs that are one   */
  /* statement plus two buttons                                          */
  /* ================================================================== */
  modal: {
    title: {
      settings: 'Settings',
      export: 'Export',
      shortcuts: 'Keyboard shortcuts',
      newProject: 'New project',
      newChapter: 'New chapter',
      deleteChapter: 'Delete chapter',
      openProject: 'Open project',
      renameProject: 'Rename project',
      removeProject: 'Remove this project?',
      formatConversion: 'Convert to an editable format?',
      cloudTransmission: 'This sends part of your page to a third party',
      cloudCost: 'Send one region to the cloud',
      overwriteRefusal: 'Refusing to overwrite the source',
    },
    meta: {
      cloudTransmission: 'first cloud request this session',
    },
    body: {
      // The surrounding pixels are the point: the
      // reconstruction steps need the ring around the text, so the crop is
      // never only the masked region, and the statement has to say so.
      cloudTransmission:
        'A bounded crop of the page, including the image content surrounding the text and not just the masked region, is transmitted to Google, processed there, and returned. Nothing else about the page or the project is sent. Local engines never leave this machine.',
      cloudCost:
        'One region, reconstructed in the cloud. Estimated {cost:currency}, billed on return; a rejected request is not billed.',
      formatConversion: {
        one: 'Convert one {from} file to {to} so it can be edited? The original stays.',
        other: 'Convert {count} {from} files to {to} so they can be edited? The originals stay.',
      },
    },
    note: {
    },
  },

  /* ================================================================== */
  /* settings - the Settings dialog's own rows                           */
  /* ================================================================== */
  settings: {
    theme: {
      label: 'Theme',
      light: 'Light',
      dark: 'Dark',
      system: 'System',
    },
    direction: {
      label: 'Reading direction',
      rtl: 'Right to left',
      ltr: 'Left to right',
    },
    cloud: {
      label: 'Cloud engines',
      description:
        // The last clause used to promise "still asks before every spend",
        // which the app does not do and never did: the cost dialog's own
        // footnote says it asks before the FIRST spend of a session and after
        // that only while Confirm before spending is on, and an Auto clean run
        // with the cloud ceiling escalates without asking at all. The blocked
        // half of this sentence is a guarantee and is kept verbatim; the
        // allowed half now claims only what allowing actually does.
        'Blocked by default. While blocked, nothing is sent to a cloud provider: the cloud rung is refused before any request is made. Allowing it makes the cloud rung available to the tools that can use it.',
      allowed: 'Allowed',
      blocked: 'Blocked',
    },
    originalView: {
      label: 'Original view',
      hold: 'Hold only',
      pinned: 'Pinned',
    },
    language: {
      label: 'Language',
      english: 'English',
    },
    sidecar: {
      label: 'AI redraw (FLUX) folder',
      chooserTitle: 'AI redraw (FLUX) folder',
      notFound: 'No engine found in this folder.',
    },
    fluxBackend: {
      label: 'AI redraw engine',
      auto: 'Automatic',
      mflux: 'MLX (Apple)',
      sdnq: 'SDNQ (any GPU)',
    },
    sidecarModel: {
      label: 'AI redraw model',
      noneFound: 'No models found in weights folder.',
    },
    // The weights and the ONNX Runtime are not bundled: they are downloaded
    // after install. Everything here is about
    // *files on this machine*, so the copy names sizes and folders and never
    // talks about "AI" - the reader is deciding what to spend disk on.
    models: {
      status: {
        installed: 'Installed',
        missing: 'Not installed',
        downloading: 'Downloading…',
        downloadingPercent: 'Downloading {percent}%',
        // The check found different bytes under the right name. Not the same
        // as "not installed": the file is there and must not be trusted.
        mismatch: 'Check failed',
        failed: 'Download failed',
        // Found outside the folder this app writes to - a bundled copy, or a
        // developer's checkout. It works; it is not ours to delete.
        readOnly: 'installed elsewhere',
        // What a stopped download left on the disk. The bytes
        // are kept so the next Download picks up where it stopped, which is
        // the half the reader has to be told before Discard means anything -
        // otherwise the button reads as "delete something I might need".
        partial: '{bytes:memory} downloaded. Download continues from there.',
      },
      action: {
        download: 'Download',
        cancel: 'Cancel',
        delete: 'Delete',
        // Throws away the unfinished download the line above reports, and
        // nothing else: the installed file, if there is one, is untouched.
        discard: 'Discard partial',
        // Re-reads the whole file and compares it against the published
        // checksum. Seconds of work on the larger models, which is why it is a
        // button rather than something that happens every time this opens.
        verify: 'Check',
      },
      runtime: {
        label: 'Engine runtime',
        // No published build for this platform. An Intel Mac, today.
        unavailable: 'Not available for this computer',
        // The build to download, where a platform publishes more than one.
        // Windows and Linux x64 do - DirectML or the WebGPU plugin for every
        // graphics card, CUDA for NVIDIA - and a Mac does not, so this row is
        // not drawn there.
        flavour: 'Runtime build',
        // The build names are ids and the versions are data, so neither is
        // translated; what needs saying is that one of the choices asks the
        // user to install something first.
        flavourNeeds: 'This build needs {items} installed on this computer before it can use the graphics card.',
        // Said **only when the two differ**. The row's build is
        // the one a Download press would fetch, and until the press happens the
        // computer is still running the one it has - two true statements that
        // read as one false one when only the first is on screen. The build
        // names and the versions are ids and data, so they are interpolated
        // rather than translated.
        installedDiffers:
          'Installed: {installed} {installedVersion}. Chosen: {chosen} {chosenVersion}. Press Download to replace it.',
      },
      // What a press answered when it did nothing. None of these is a failure:
      // each is this window looking at a row that another window has already
      // moved past.
      declined: {
        alreadyRunning: 'This is already downloading.',
        alreadyInstalled: 'This is already installed.',
        notFound: 'There was nothing to delete.',
        readOnlyElsewhere: 'This copy is outside the folder this app manages, so it was left alone.',
        // A delete refused because a download is writing into the same folder.
        // The only row that can reach it is the runtime's.
        busy: 'A download is using this folder. Cancel it first, then delete.',
      },
      folder: 'Downloads go to {path}',
      unavailable: 'This build cannot manage downloads.',
      token: {
        label: 'Hugging Face token',
        placeholder: 'hf_…',
        save: 'Save',
        clear: 'Clear',
        saved: 'A token is saved in this computer’s credential store.',
        // The fallback, in its two forms. They read almost the
        // same and mean opposite things: one is a computer that cannot do
        // better, the other is a keychain that would work if it were unlocked.
        // Saying the first when it is the second sends the
        // user looking for a problem that is not there.
        savedInFile: 'A token is saved. This computer has no credential store, so it is kept in the settings file as plain text.',
        savedStoreUnreachable: 'A token is saved. This computer’s credential store could not be reached, so it is kept in the settings file as plain text. Unlock the keychain and save again to move it.',
        fileOnly: 'This computer has no credential store, so a token would be kept in the settings file as plain text.',
        storeUnreachable: 'This computer’s credential store could not be reached, so a token would be kept in the settings file as plain text.',
        // A Clear the credential store refused. The secret is still in it, and
        // the only place it can be removed from is the user's own keychain.
        clearFailed: 'The token could not be removed from this computer’s credential store. It is still there. Remove it with the system keychain tool.',
        // Every other way a Clear can fail: an unwritable settings file, a
        // backend that is not answering. The keychain sentence above is a
        // specific and alarming claim - "your secret is still there" - and
        // saying it for a disk error sends the user to the wrong tool.
        clearFailedOther: 'The settings could not be written, so nothing was changed. Try again.',
        // **Why** the store could not be reached, in the user's terms rather
        // than the platform's. One line under the sentence
        // above, and each one is a different instruction: the first is the only
        // one the user can act on where they are standing.
        reason: {
          locked: 'It looks locked. Unlock it and press Save again.',
          unreachable: 'The credential service did not answer on this computer.',
          ambiguous:
            'It holds more than one entry for this application and cannot tell which token is this one’s. Remove the extra entries with the system keychain tool.',
          unknown: 'It refused without saying why.',
        },
      },
    },
    accel: {
      label: 'Graphics acceleration',
      auto: 'Automatic',
      // The picker with nothing in it. `listAccelerators` is the engine runtime
      // being asked what this machine can run a model on, so a rejection is
      // almost always the runtime itself failing to load - and the runtime's
      // own row under Models is already where the reason for that is written.
      // This sentence sends the reader there rather than restating it, because
      // two explanations of one fault are two things that can disagree.
      unreadable:
        'The accelerator list could not be read, so only Automatic is offered. It comes from the engine runtime, so the reason for this will be on the runtime’s row under Models.',
    },
    // Which modifier is held while clicking the page to set where Clone / heal
    // reads from. The *caps* are not here: `⌥` and `Alt` are the keys' own
    // names, printed on the hardware, and `src/lib/shortcuts.js` supplies them
    // per platform the way it supplies every other keycap. What is here is the
    // spelt-out name, which is what a screen reader says and what the tooltip
    // shows - and it names both keyboards, because one row is read on both.
    cloneSource: {
      label: 'Clone / heal source',
      name: {
        alt: 'Option, or Alt',
        meta: 'Command, or the Windows key',
        control: 'Control',
        shift: 'Shift',
      },
    },
    // The tab strip. `general` heads the five preference rows, which had no
    // heading at all while they were simply the top of the scroller.
    section: {
      general: 'General',
      models: 'Models',
      acceleration: 'Acceleration',
      shortcuts: 'Shortcuts',
      about: 'About',
    },
    tabs: {
      label: 'Settings sections',
    },
  },

  /* ================================================================== */
  /* about - the GPL-3.0 obligations                                      */
  /* ================================================================== */
  about: {
    fact: {
      version: 'Version',
      licence: 'Licence',
      source: 'Source',
      detector: 'Detection',
      engines: 'Inpainting',
      cloud: 'Cloud',
      runtime: 'Runtime',
    },
    offer: {
      // The GPL-3.0 written offer. Not decoration: §6 of the licence requires
      // it wherever the corresponding source is not shipped alongside.
      written:
        'Manga Cleaner is free software under the GNU General Public License, version 3 or later. The complete corresponding source is available at the address above, and will be supplied on physical media on request for no more than the cost of distribution.',
    },
    note: {
      // Names Google, as `modal.body.cloudTransmission` does. About is where a
      // user reads about the cloud when they are *not* mid-request, so it is
      // the worse of the two places to leave the provider unnamed.
      cloudTerms:
        'Cloud requests go to Google and are governed by Google’s own terms, not by this licence. They are opt-in for every request, and they send a bounded crop of the page, including the image content around the text, off this machine.',
    },
  },

  /* ================================================================== */
  /* export - the Export dialog and the overwrite refusal                */
  /* ================================================================== */
  export: {
    meta: {
      chapter: {
        one: '1 page · Ch. {chapter}',
        other: '{count} pages · Ch. {chapter}',
      },
    },
    // JPEG is deliberately absent. Lossy formats belong below the
    // fidelity line, behind an acknowledgement that does not exist here -
    // and a button that always refuses is worse than no
    // button. `notice.export.refusedLossyFormat` is what a caller that asks for
    // one anyway is told.
    format: {
      label: 'Format',
      png: 'PNG',
      tiff: 'TIFF',
      psd: 'PSD',
      cbz: 'CBZ',
    },
    layout: {
      label: 'Layout',
      perPage: 'One file per page',
      stitched: 'One file for the chapter',
    },
    destination: {
      label: 'Destination',
      newFolder: 'New folder',
      sourceFolder: 'Source folder',
      customFolder: 'Another folder',
      pathLabel: 'Export folder',
      pathPlaceholder: '/Users/you/manga/ch01_cleaned',
      chooserTitle: 'Choose a folder to export into',
    },
    masks: {
      label: 'Masks',
      flattened: 'Flattened',
      separateLayer: 'Separate layer',
    },
    note: {
      // Pages narrower than the strip get columns beside them
      // that no source file has a pixel for, and an interface offering
      // stitched output states the count *before* it runs - the same standard
      // every other unasked-for change is held to.
      gutter: {
        zero: 'Every page is the full width of the chapter, so the single file invents no pixels.',
        one: '1 pixel of paper white will be written beside the narrower pages. No source file has a pixel there.',
        other:
          '{count} pixels of paper white will be written beside the narrower pages. No source file has a pixel there.',
      },
      flagged: {
        zero: 'Nothing is flagged for review.',
        one: '1 region is still flagged for review. It exports as it is now.',
        other: '{count} regions are still flagged for review. They export as they are now.',
      },
    },
    state: {
      exporting: 'Exporting…',
    },
    action: {
      export: 'Export {format}',
      chooseAnotherFolder: 'Choose another folder…',
    },
    refusal: {
      body: 'Export would replace the files in {path}. Cleaned pages must be written somewhere else. Choose another folder and the export will run.',
    },
    // Not an error. The UI never silently downgrades, so a format
    // that cannot carry the source's colour mode produces a written statement
    // of exactly what will change, before the export runs.
    declared: {
      formatChanged:
        'Saved as {used} rather than {requested}: {requested} has no {mode} mode, and converting would change the file.',
    },
  },

  /* ================================================================== */
  /* home - library, chapters, and Home's four dialogs                   */
  /* ================================================================== */
  home: {
    section: {
      projects: 'Projects',
      chapters: 'Chapters',
    },
    action: {
      newProject: 'New project',
      newChapter: 'New chapter',
      deleteChapter: 'Delete chapter',
      chapterMenu: 'Actions for {name}',
      open: 'Open',
      rename: 'Rename',
      remove: 'Remove',
      retry: 'Try again',
      settings: 'Settings',
      continueClean: 'Continue cleaning',
      backToProjects: 'Back to projects',
      copySourcePath: 'Copy source path',
    },
    menu: {
      project: 'Actions for {name}',
    },
    hint: {
      newChapterNeedsProject: 'Create a project first. A chapter belongs to one.',
    },
    project: {
      meta: {
        select: 'chapters',
        one: '1 chapter · {pages} pages · {modeKey}',
        other: '{chapters} chapters · {pages} pages · {modeKey}',
      },
    },
    card: {
      chapterLine: {
        one: 'Ch. {chapter} · {title}',
        other: 'Ch. {chapter} · {title} · {count} chapters',
      },
      cleaned: '{cleaned} of {total} pages cleaned',
      resumeAt: 'Interrupted in Ch. {chapter}, page {page}',
    },
    chapter: {
      number: 'Ch. {number}',
      pages: {
        one: '1 page',
        other: '{count} pages',
      },
      positions: {
        one: '1 position',
        other: '{count} positions',
      },
      cleaned: '{cleaned} of {total} cleaned',
      needReview: {
        one: '1 needs review',
        other: '{count} need review',
      },
      skipped: {
        one: '1 skipped',
        other: '{count} skipped',
      },
      interrupted: 'Interrupted at page {page}',
    },
    empty: {
      libraryTitle: 'No projects yet',
      chaptersTitle: 'No chapters yet',
    },
    state: {
      loading: 'Loading the library…',
      failedTitle: 'The library could not be read',
      failedBody: 'Nothing has been changed. Try again, or check that the library folder is still where it was.',
      projectMissingTitle: 'That project is no longer in the library',
      projectMissingBody: 'It may have been removed. The pages on disk are untouched either way.',
    },
    newProject: {
      source: 'Source folder',
      sourcePlaceholder: 'Choose a folder…',
      name: 'Project name',
      namePlaceholder: 'Untitled project',
      mode: 'Page mode',
      modeNote:
        'Set once, at creation. It decides how pages are stitched and split in every chapter of this project, and it cannot be changed afterwards.',
      permanent: 'Page mode is permanent.',
      createSingle: 'Create single-page project',
      createLongstrip: 'Create longstrip project',
    },
    newChapter: {
      project: 'Project',
      noProject: 'No project selected',
      source: 'Source folder',
      sourcePlaceholder: 'The project’s folder…',
      // The copy is the point: it is what tells a user the folder is read once
      // and can then be moved or deleted.
      sourceNote:
        'The pages are copied into this app and never written back. You can move or delete the folder afterwards.',
      number: 'Chapter number',
      taken: 'Chapter {number} already exists in this project.',
      title: 'Chapter title',
      defaultTitle: 'Chapter {number}',
      create: 'Add Ch. {number}',
    },
    openProject: {
      filter: 'Filter projects',
      filterPlaceholder: 'Type a project name',
      noMatches: 'No project matches that.',
      meta: {
        select: 'chapters',
        one: '1 chapter · {pages} pages',
        other: '{chapters} chapters · {pages} pages',
      },
    },
    deleteChapter: {
      body: 'Delete Ch. {number} “{name}”? Its cleaning is lost. The scans in {path} stay unless you choose to remove them too.',
      // A chapter with no folder of its own reads the project's folder, which
      // the project's other chapters read too - so the scans cannot be offered
      // here at all, and the copy says which folder it is protecting.
      bodyOwnFolder: 'Delete Ch. {number} “{name}”? Its cleaning is lost. The scans stay: this chapter reads the project’s own folder.',
      keepScans: 'Delete chapter',
      withScans: 'Delete chapter and scans',
    },
    rename: {
      label: 'Project name',
    },
    remove: {
      // The scans stay, and since the library keeps its own copy of every page
      // that is now the smaller half of the truth: what goes is a copy the user
      // may have deleted their originals for. Both halves, in that order.
      body: 'Remove {name} from the library? Its cleaning and its copy of the pages go. The scans in {path} stay.',
    },
  },

  /* ================================================================== */
  /* project                                                             */
  /* ================================================================== */
  project: {
    mode: {
      single: 'single page',
      longstrip: 'longstrip',
    },
  },

  /* ================================================================== */
  /* editor - the chrome around the canvas                               */
  /* ================================================================== */
  editor: {
    region: {
      identity: 'Project and chapter',
      windows: 'Panels',
      view: 'View',
      tools: 'Tools',
      viewControls: 'View controls',
      chapterNav: 'Pages and review',
      reviewSet: 'Regions needing review',
      canvas: 'Page',
    },
    identity: {
      chapter: '· Ch. {number}',
    },
    direction: {
      rtl: 'right to left',
      ltr: 'left to right',
    },
    panel: {
      pages: 'Pages',
      // The window's own heading, and the name its move / collapse / close
      // buttons take. The design file titles this window LAYERS; `Layers &
      // review` is the panel toggle's name (`shortcuts.panel.masks`), where
      // there is room for it. In a 248px header the long form left 43px for
      // the meta, which rendered `no masks` as `no ma…` and `2 flagged` as
      // `2 flag…`.
      layers: 'Layers',
    },
    meta: {
      pagesCleaned: '{done} of {total} cleaned',
      masksApplied: {
        zero: 'no masks',
        one: '1 mask',
        other: '{count} masks',
      },
      masksFlagged: {
        zero: 'nothing flagged',
        one: '1 flagged',
        other: '{count} flagged',
      },
    },
    action: {
      home: 'Library',
      settings: 'Settings',
      export: 'Export',
      renameProject: 'Rename project',
      pagesWindow: 'Pages',
      layersWindow: 'Layers & review',
      maskOverlay: 'Mask overlay',
      originalHold: 'Hold to show the original',
      originalPin: 'Pin the original',
      wipe: 'Wipe between original and cleaned',
      zoomIn: 'Zoom in',
      zoomOut: 'Zoom out',
      zoomFit: 'Fit the page',
      prevReview: 'Previous region needing review',
      nextReview: 'Next region needing review',
      undo: 'Nothing to undo',
      redo: 'Nothing to redo',
      undoCommand: 'Undo {commandKey}',
      redoCommand: 'Redo {commandKey}',
      runOnPage: 'Run on page',
      runOnProject: 'Run on project',
      cancelRun: 'Cancel run',
    },
    readout: {
      page: 'Page {index} of {total}',
      review: 'Region {index} of {total}',
      reviewPending: {
        select: 'total',
        zero: 'Nothing to review',
        one: '1 to review',
        other: '{total} to review',
      },
      zoomActual: '{percent}% · click for 1:1',
      zoomActualFromFit: 'Fitted · click for 1:1',
    },
    zoom: {
      fit: 'Fit',
    },
    // Four characters at most: the readout box is 30px wide. The full words
    // are spoken through the slider's aria-valuetext, not printed here.
    wipe: {
      clean: 'clean',
      original: 'orig',
    },
    window: {
      titleBar: '{windowName} window',
      move: 'Move {windowName}',
      resize: 'Resize {windowName}',
      collapse: 'Collapse {windowName}',
      expand: 'Expand {windowName}',
      close: 'Close {windowName}',
    },
    run: {
      progress: '{done} of {total}',
    },
    status: {
      // Beside the Run button while a run is going, in a live region. There is
      // no idle counterpart any more: the tool bar replaced the tool window,
      // and the two "click a region to apply X" lines it used to carry were a
      // footer explaining a tool the user had just chosen from the rail
      //.
      cleaning: 'Cleaning page {page}. Cancel from Auto clean.',
    },
    state: {
      opening: 'Opening the chapter…',
      noPages: 'This chapter has no pages.',
      cloudBlocked: 'Cloud engines are blocked in Settings.',
      // Auto clean's three files are missing. Disabled with a sentence rather
      // than hidden: an engine option has four alternatives beside it and this
      // button has none, so a tool that quietly lost its only action would
      // read as a broken window rather than as a missing download.
      modelsMissing: 'Auto clean needs its models. Download them in Settings › Models.',
      runtimeMissing: 'No engine runtime found. Download it in Settings › Models.',
    },
  },

  /* ================================================================== */
  /* canvas - the page surface                                           */
  /* ================================================================== */
  canvas: {
    region: {
      name: '{titleKey} · {statusKey}',
      nameFlagged: '{titleKey} · {statusKey}. {reasonKey}.',
    },
    action: {
      clearSelection: 'Page: activate to clear the selection',
      draw: 'Drawing surface: activate to open a draft region',
      drawActive: 'Draft region: arrows move, Shift and arrows resize, Enter commits, Escape abandons',
    },
    draft: {
      size: '{w} × {h}%',
    },
    command: {
      // One label for four applications (a region click, a content-aware fill,
      // an AI snap onto an existing region, a cloud fill). It says what the
      // undo entry undoes without claiming which tool made it - the tool is
      // already named in the Layers row this entry came from.
      applyTool: 'the tool applied to a region',
      drawMask: 'a mask drawn by hand',
    },
  },

  /* ================================================================== */
  /* pages - the Pages list                                              */
  /* ================================================================== */
  pages: {
    label: {
      page: 'p. {number}',
      position: 'pos {number}',
    },
    status: {
      unclean: 'not cleaned',
      cleaning: 'cleaning now',
      cleaned: 'cleaned',
      cleanedNeedsReview: 'cleaned, needs review',
      skipped: 'skipped',
    },
    row: {
      // `count` is the number of regions still needing review, and it is 0 on
      // most rows - which is why the zero form drops the clause entirely
      // rather than saying "0 regions need review".
      name: {
        zero: '{label} · {statusKey}, {cleaned} of {total} regions',
        one: '{label} · {statusKey}, {cleaned} of {total} regions, 1 needs review',
        other: '{label} · {statusKey}, {cleaned} of {total} regions, {count} need review',
      },
      skipped: '{label} · skipped: {reasonKey}',
    },
  },

  /* ================================================================== */
  /* paging                                                              */
  /* ================================================================== */
  paging: {
    action: {
      prev: 'Previous page',
      next: 'Next page',
    },
  },

  /* ================================================================== */
  /* progress - the Home rollups                                         */
  /* ================================================================== */
  progress: {
    status: {
      notStarted: 'Not started',
      inProgress: 'In progress',
      review: 'Needs review',
      completed: 'Completed',
    },
  },

  /* ================================================================== */
  /* review - why a region is flagged                                    */
  /* ================================================================== */
  review: {
    reason: {
      fittingReconstructed: 'Fitting failed, so a model reconstructed the area',
      unusuallyLarge: 'Unusually large for this page',
      declined: 'Every engine failed the quality check, so the original text was left alone',
      gateSkippedLowConfidence: 'The script gate was not confident enough to clean it',
      gateSkippedOutsideBubble: 'Text outside a speech bubble',
      // Not a failure of the gate - the opposite. It read the script, the
      // script was not Japanese, and leaving Latin text alone is the product.
      // Distinct from `low-confidence` because
      // reporting a confident refusal as an uncertain one is backwards.
      gateSkippedNotJapanese: 'Not Japanese, left as it was',
      cloudAccepted: 'A cloud request was accepted, so check it against the page',
      cloudRejectedSafetyFilter: 'the provider’s safety filter refused it',
      cloudRejectedTransportError: 'the request never completed',
      cloudRejectedParameterTest: 'it failed the parameter test',
      cloudRejectedResidualTest: 'it failed the residual test',
      cloudRejectedStructural: 'it failed the structural test',
    },
  },

  /* ================================================================== */
  /* decline / input - terminal outcomes the pipeline reports            */
  /* ================================================================== */
  decline: {
    reason: {
      qualityMetric: 'no engine met the quality metric',
      // The metric has two halves, and a reader acting on
      // them acts differently: the first says the engine put something on the
      // paper, the second says it put nothing recognisable as that paper. One
      // is a reason to look at the region, the other is a reason to look at the
      // page it was cut from. `qualityMetric` above is the pair of them said at
      // once, which is all the core could say while these two did not exist.
      edgeEnergy: 'every engine left strokes the paper around it does not have',
      histogram: 'no engine matched the tone of the paper around it',
      // Past four model inputs a region is declined
      // rather than tiled, because leaving text is recoverable and a bad fill
      // is not.
      tooLarge: 'the region is larger than the inpainter can cover',
      // Indexed and CMYK sources
      // cannot carry a model rung's output at all.
      unrepresentableMode: 'this file’s colour mode cannot hold an inpainted result',
      paintUnsupportedMode: 'this file’s colour mode cannot hold a painted or cloned result',
      unrepresentableDepth: 'this file has one bit per sample, which an inpainted result cannot hold',
      // Rung 3a only, and a narrower fact than `unrepresentableDepth` above:
      // the file's samples are perfectly representable, and it is that one
      // engine which is not built to carry them. The AI redraw helper works in
      // 8 bits per sample (`engines::flux::declines`), so a 16-bit source that
      // the default inpainter handles at full depth cannot go to it. The remedy
      // is to use the default inpainter, which is why this does not read as a
      // property of the file.
      //
      // It said "this fast preview engine" until MI-GAN was removed. The rung
      // is gone; the sentence was still being emitted, by rung 3a, under the
      // departed rung's description.
      depthBeyondEngine: 'the AI redraw helper works at 8 bits, and this file has more',
      // Not a judgement on the region at all - no engine was ever asked about
      // it. Two ways in, and the remedy differs: the engine ceiling was set
      // below the rung the region needed, or this machine has no weights for
      // that rung. Distinct from the four above, each of which is something an
      // engine decided after looking.
      rungUnavailable: 'the engine this region needs was not available',
      // The engine was asked, it started, and then it stopped answering: an
      // ONNX session that built and ran and died under it, which on Windows is
      // usually the graphics driver being reset out from under the run. Said
      // as a fact about the model rather than about the region, because the
      // region is fine and the next attempt on a fresh session may well work.
      // The remedy is not here: it is `notice.run.engineFault`, which names
      // the model and the provider and says what to change. Until this key
      // existed the edit rejected with ONNX Runtime's own string, which no
      // catalogue lookup catches, so the click did nothing visible at all.
      engineFault: 'the model stopped answering',
      // Rung 3a's four. None of them
      // is a fact about the region - they are all facts about the machine or
      // the process, which is what separates them from the six above and why
      // each names a different remedy. There is no key for the sidecar simply
      // not being installed: that is the ordinary state, it is reported as
      // nothing at all, and a rung the user never installed is not a failure
      // the user has to dismiss.
      sidecarMachine:
        'this machine does not have the memory the FLUX sidecar needs for this model',
      // A probe that was made and failed - a sysctl, a /proc read, a
      // GlobalMemoryStatusEx. This used to be every Windows and Linux machine;
      // now both are measured, and what stops them is the row
      // below. A different sentence from the one above because the remedy is
      // different - this is the application's gap, not the machine's.
      sidecarUnknownMachine:
        'this machine’s memory could not be measured, so the FLUX sidecar is not offered here',
      // The *chosen* backend has no render path here. It used to be every
      // Windows and Linux machine, because mflux was the
      // only rendering backend and MLX is Apple Silicon; since the sdnq backend
      // there is no platform without one, so what reaches this is a user who
      // picked mflux on a machine with no MLX. Not a fact about the machine's
      // memory and deliberately not phrased as one.
      sidecarPlatform:
        'this build has no FLUX sidecar for this kind of computer, so the helper is not offered here',
      // Installed, and installed without the chosen engine's Python packages -
      // a virtual environment may hold either backend's. Its own sentence
      // because its own remedy is one command, unlike the row above.
      sidecarBackend:
        'this FLUX sidecar folder does not have the chosen engine installed',
      // The third guard doing its job: the sidecar hit the ceiling
      // it was given and raised, rather than driving the machine into swap.
      sidecarMemory: 'the FLUX sidecar reached the memory it was allowed',
      // The first guard: "adding a backend means adding its bound, or
      // refusing to offer it".
      sidecarUnbounded:
        'this FLUX sidecar cannot say how much memory it needs, so it is not run',
      // Almost absence, and deliberately not silent like absence: this is a
      // user who installed the sidecar and is one download away from the rung.
      sidecarWeights: 'the FLUX sidecar is installed and has no model weights yet',
      // The one entry in this group the core never chooses: the interface does,
      // when a region edit is rejected with something that is not a key at all.
      // The backend answers most failures with one of the reasons above, and it
      // can still reject with a sentence of its own - a disk that would not
      // write, a library's own words - and putting that on screen is how a
      // reader ends up looking at an ONNX Runtime stack trace in a notice. This
      // says the true and useful part instead, and the sentence around it says
      // the region is untouched, which is the half that matters. The raw text
      // is not lost: it goes to the console, where the other unreadable
      // diagnostics go.
      unknown: 'the engine failed for a reason this build does not recognise',
    },
  },
  // Why a file did not make it into a chapter. The
  // rule is that a skip is always *listed*, never silent, so each of these is
  // read by a user deciding whether the file matters.
  input: {
    skipReason: {
      truncatedJpeg: 'the file is a truncated JPEG',
      notAnImage: 'it is not an image this reads',
      headerUnreadable: 'its header does not parse',
      // The truncated-scan case §2 singles out: the header parsed and the
      // pixels did not, which is exactly when a partial decode would produce a
      // "cleaned" grey tail.
      partialDecode: 'it does not decode completely',
      unsupportedMode: 'its colour mode has no policy here, and promoting it would change the file',
      duplicate: 'another file has the same contents',
      junk: 'it is a system file',
      // The file was fine and this application's own write of the PNG made
      // from it was not - a full disk, a read-only library. Said separately
      // from `partialDecode` so nobody goes looking at the scan.
      conversionFailed: 'it could not be saved as a PNG',
      // The same failure for a file that needed no converting: the page is a
      // PNG or TIFF already and the library's own copy of it could not be
      // written. Separate from `conversionFailed` so the sentence is not about
      // a conversion that never happened.
      importFailed: 'it could not be copied into the library',
    },
  },

  /* ================================================================== */
  /* diagnostics - why the ONNX Runtime is not usable                    */
  /* ================================================================== */
  // Five states, told apart because their remedies are: download it, clear an
  // extended attribute, re-sign the application, install a Microsoft
  // redistributable, replace the file.
  diagnostics: {
    runtime: {
      missing: 'The ONNX Runtime was not found',
      quarantined: 'The ONNX Runtime is quarantined, so the system refused to load it',
      refused: 'The system refused to load the ONNX Runtime into this application',
      // Windows, and the one of the five that is not about our file at all: the
      // `onnxruntime.dll` we ship imports `MSVCP140.dll`, `MSVCP140_1.dll`,
      // `VCRUNTIME140.dll` and `VCRUNTIME140_1.dll`, which arrive with the
      // Microsoft redistributable and with nothing else. Without it the load
      // fails with Windows error 126, which read as `unloadable` until now -
      // and that sent the reader to download our file again, when our file was
      // never the thing that was wrong. The product is named in full because
      // the full name is what has to be searched for to fix this.
      missingDependency:
        'The Microsoft Visual C++ 2015-2022 Redistributable (x64) is not installed, so the ONNX Runtime cannot load. Install it from Microsoft and start Manga Cleaner again',
      unloadable: 'The ONNX Runtime could not be loaded',
    },
  },

  /* ================================================================== */
  /* accel - which execution provider a model ran on                     */
  /* ================================================================== */
  accel: {
    cpu: 'CPU',
    coreml: 'CoreML',
    directml: 'DirectML',
    cuda: 'CUDA',
    tensorrt: 'TensorRT',
    rocm: 'ROCm',
    openvino: 'OpenVINO',
    webgpu: 'WebGPU',
    xnnpack: 'XNNPACK',
    // Not an execution provider: rung 3a runs in a separate program, so "where
    // is it running" is answered by naming the program rather than a provider
    // inside this one (cleaner_core::registry::Device::sidecar).
    sidecar: 'Helper app',
    declined: {
      unavailable: 'not available in this runtime',
      // A provider without a `DFT` kernel does not fail, it makes
      // the graph split and run slower.
      partitioned: 'it would split this model and run it slower',
      failed: 'it could not build a session for this model',
      // Forcing CoreML costs 8.19 GB on rung 2 and 5.62 GB on
      // rung 3, and this is the one refusal that is about the machine rather
      // than about the run. The figures travel beside the key, on the
      // selection, because a byte count is not translatable.
      memory: 'this machine has less memory than that provider needs for this model',
      // The other absence, and it is a different remedy: the provider is in the
      // downloaded runtime and the parts NVIDIA ships - the CUDA runtime,
      // cuDNN - are not on this machine. `unavailable` above would send someone
      // to re-download the one thing that is already right.
      missingRuntime: 'it needs CUDA and cuDNN installed on this machine',
      // The third absence: the WebGPU plugin is loaded and
      // found no graphics device. A driver rather than a download - on Linux
      // the Vulkan loader, `libvulkan.so.1`, which every GPU driver package
      // carries; on Windows a working graphics driver.
      noDevice: 'it found no graphics device, so install your graphics driver (on Linux, the Vulkan loader)',
    },
    chosen: {
      // The second tier. A guess stated as a guess: the alternative is
      // writing a guess down as a measurement, which is the error the log
      // otherwise records.
      unmeasured: 'chosen without a measurement on this hardware',
    },
  },

  /* ================================================================== */
  /* models - what is loaded right now, in the corner of the editor      */
  /* ================================================================== */
  //
  // The names are what the thing *does for the reader*, not what it is:
  // nobody outside this repository knows what a segmentation model or an
  // inpainter is, and the tab exists so that a person watching their machine
  // slow down can see what is using the memory and stop it. `models.kind.*`
  // matches `cleaner_core::registry::Kind::label_key` key for key.
  models: {
    // Not "loaded models": the word the row is about is memory, which is what
    // the reader came here worried about.
    title: 'Using memory now',
    kind: {
      textDetector: 'Text finder',
      balloonDetector: 'Speech bubble finder',
      scriptGate: 'Language checker',
      // Its labels ship as a second file and the two must match: a gate with
      // the wrong labels is not a gate. Named separately because Settings
      // lists one row per *file*, and a row with no name is a row nobody can
      // decide about.
      scriptGateLabels: 'Language checker labels',
      // The gate's rescue reader. Named for what it does rather
      // than for what it is: it reads the Japanese in a balloon the language
      // checker could not make out, so that an ordinary line of dialogue is
      // cleaned instead of landing in review. Three files, three rows in
      // Settings, one name between them plus two that say which part - the
      // same shape the language checker and its labels have.
      ocr: 'Japanese text reader',
      ocrDecoder: 'Japanese text reader, second part',
      ocrVocab: 'Japanese text reader characters',
      // Rung 2. "Redraw" is the word the tools already use for what an
      // inpainter does to the paper under the text.
      inpainter: 'Redraw model',
      // Rung 3a, which is a separate program on the machine and is the only
      // row that can be holding several gigabytes.
      sidecar: 'AI redraw helper',
    },
    value: {
      // `{bytes:memory}` - see `FORMATS.memory` in src/lib/i18n/index.js.
      size: '{bytes:memory}',
      // The same figure when the row's `basis` is not a measurement of a
      // running session - the size of the `.onnx` on disk for four of the six
      // rows, which is a *floor*: a session is the weights plus the execution
      // provider's arena. The tilde is the
      // whole of the correction, and it is the honest one: inventing a
      // per-provider coefficient would dress an estimate up as the measurement
      // this row does not have.
      sizeApprox: '~{bytes:memory}',
      // The row while the request is in flight. Unloading happens between
      // regions, so a click during a busy page waits a moment, and saying so
      // is better than a button that appears to have done nothing.
      unloading: 'Freeing…',
    },
    action: {
      // A per-row button, so the name of what is being closed is in the label
      // rather than only in the row above it.
      unload: 'Free the memory {nameKey} is using',
    },
    hint: {
    },
    /* The offer made on a first launch. The
       downloader has always been there; until this it was one the user had to
       go and find, and a fresh install opened an editor whose only cleaning
       action was disabled. Every sentence here names Settings › Models,
       because that is the way back for a user who declines, cancels, or is
       interrupted. */
    firstLaunch: {
      title: 'Download the engines?',
      description: 'The engines download once and stay on this computer. Anything skipped here can be added later in Settings › Models.',
      requiredLabel: 'Needed to clean a page',
      // A platform with no published ONNX Runtime - an Intel Mac. The weights
      // are still offered, because they are what an offline install needs
      // beside a library placed by hand, and this says why they are not enough
      // on their own.
      runtimeUnavailable:
        'There is no engine runtime published for this computer, so nothing here can run until one is installed by hand. The files below are still worth having: they are what such an install needs beside it.',
      requiredNote: '{bytes:memory} in all. Auto clean stays disabled until these are here.',
      optionalLabel: 'Redraw engine',
      // One engine rather than two since MI-GAN was removed, so the sentence
      // no longer compares a pair. What is left is the only thing a user
      // actually has to decide
      // here: what this download buys them, and what they give up by skipping
      // it. The size is on the row itself and is not repeated.
      optionalNote: 'Rebuilds the artwork under the text. Without it, anything over artwork or screentone is left for you.',
      // A failure stops the sequence rather than carrying on into the next
      // download: the artefacts are wanted together, and four failures in a row
      // read as a broken application rather than as one bad connection.
      failed:
        '{nameKey} could not be downloaded, so the rest were not started. Settings › Models can try again.',
      stopped: 'Stopped. Settings › Models is where the rest are downloaded.',
      done: 'Everything chosen is installed.',
      action: {
        download: 'Download {bytes:memory}',
        // Not "Cancel": nothing is being undone, and the offer is not a
        // question the user has to answer now.
        notNow: 'Not now',
      },
    },
  },

  /* ================================================================== */
  /* ladder - the engine rungs                                           */
  /* ================================================================== */
  ladder: {
    rung: {
      fill: 'Planar fill',
      denoise: 'Denoise',
      lama: 'manga-LaMa',
      // The optional rung 3a. Never bundled, never
      // on the default path - but a patch it produced still names it, so a
      // project made on a machine with the sidecar reads correctly on one
      // without it.
      flux: 'FLUX',
      cloud: 'Cloud',
      paint: 'Paint',
      clone: 'Clone',
      unknown: 'Unknown engine',
    },
  },

  /* ================================================================== */
  /* masks - the Layers & review window                                  */
  /* ================================================================== */
  masks: {
    filter: {
      needsReview: 'Needs review',
      hint: 'Show only the regions that need review (R)',
    },
    scope: {
      viewport: 'Positions {from} to {to}',
      viewportOne: 'Position {position}',
    },
    empty: {
      noMasks: 'No masks on this page yet.',
      noneNeedReview: 'Nothing here needs review.',
      noText: 'No text was detected in this chapter.',
    },
    title: {
      declined: 'Left as it was',
      gateSkipped: 'Held back by the script gate',
      noMask: 'No mask',
    },
    status: {
      // The four states `maskRow()` classifies a region into. `applied` names
      // what it is again, now that `unexamined` carries the case it used to be
      // stretched over: a region with no mask that nothing has declined, held
      // back or flagged. `canvas.region.name` renders these as
      // `{titleKey}, {statusKey}`, so each has to be true beside its title.
      applied: 'Applied',
      needsReview: 'Needs review',
      declined: 'Declined',
      unexamined: 'Not cleaned yet',
    },
    sub: {
      noMask: 'nothing applied',
    },
    origin: {
      hand: 'hand',
      auto: 'automatic',
    },
    fillMode: {
      matchSurround: 'Match surround',
      reconstruct: 'Reconstruct',
      solid: 'Solid',
    },
    provenance: {
      engine: 'Engine',
      modelVersion: 'Model',
      fillMode: 'Fill',
      elapsed: 'Elapsed',
      cloudCost: 'Cost',
      cloudRequestId: 'Request',
      origin: 'Origin',
      flagged: 'Flagged',
      applied: 'Applied',
      detected: 'Detected',
    },
    value: {
      elapsed: '{ms} ms',
      // The only currency in the app. `currency` runs Intl.NumberFormat; no
      // other string may write a `$`.
      cloudCost: '{cost:currency}',
      nothingApplied: 'nothing (the original text is untouched)',
      detectedYes: 'yes, by the automatic pass',
      detectedNo: 'no, the automatic pass missed it',
    },
    // **The models themselves.** These used to be plain verbs - "Quick",
    // "Redraw, stronger" - on the reasoning that a translator should never
    // have to read the ladder's own words. The user ruled the other way:
    // an engine picker exists so that
    // somebody can move between *models*, and a name that hides which model
    // ran makes that impossible. Short names, so a row and a segmented control
    // both have room for them.
    engineChoice: {
      fill: 'Fill',
      denoise: 'Denoise fill',
      lama: 'LaMa',
      // Rung 3a, offered only where the sidecar is
      // actually installed - `rowEngines` in `src/lib/model/masks.js`.
      flux: 'FLUX',
      // Not offered by a row's picker - see `ROW_ENGINES` in
      // `src/lib/model/masks.js` - but a mask that already ran on the cloud
      // still has to be able to name its own entry.
      cloud: 'Cloud',
    },
    action: {
      delete: 'Delete layer',
      deleteHint: 'Delete this layer and bring the original text back',
      deleteRegion: 'Dismiss',
      deleteRegionHint: 'Take this off the list and leave the page as it is',
      retry: 'Try again',
      retryHint: 'Clean this area again with the same setting',
      engine: 'Clean with',
      engineHint: 'Clean this area again with a different setting',
      showOnPage: 'Show on page',
      cleanAnyway: 'Clean anyway',
    },
    menu: {
      // The right-click menu's own name, for a screen reader announcing it.
      // "Layer" rather than "region" or "mask": the panel is called LAYERS and
      // the menu is raised on a row of it, or on the box the row stands for.
      label: 'Layer actions',
    },
    hint: {
      showOnPage: 'Scroll to this region and select it',
      cleanAnyway: 'Clean it despite the script gate',
    },
    command: {
      deleteMask: 'a mask deleted',
      deleteRegion: 'a region dismissed',
      rerunMask: 'a mask re-run',
      cleanAnyway: 'a gate-skipped region cleaned',
    },
  },

  /* ================================================================== */
  /* tools - the tool rail and the tool bar                             */
  /* ================================================================== */
  tools: {
    name: {
      autoClean: 'Auto clean',
      brush: 'Brush',
      shapes: 'Shapes',
      aiMaskBrush: 'AI mask brush',
      contentAwareFill: 'Content-aware fill',
      cloneHeal: 'Clone / heal',
    },
    // The tooltip on the tool's own name at the left of the bar. It used to be
    // the window header's right-aligned meta; a bar has no room for a second
    // line of text beside the name, and a hint is the one thing here a tooltip
    // is honestly enough for - it is a reminder of the gesture, not a reason
    // anything is unavailable.
    hint: {
      autoClean: 'Enter to run',
      brush: 'drag to paint',
      shapes: 'drag on the page',
      aiMaskBrush: 'stroke over text',
      contentAwareFill: 'click a mask',
      // `{cloneSourceModifier}` is a context param, not one this call site
      // passes: the tool bar renders `t(spec.hintKey)` over a key the tool
      // table chose, and the modifier is a preference. See `provideContextParam`.
      cloneHeal: '{cloneSourceModifier}-click a source',
    },
    // There is no `note` family. Every tool used to carry a sentence or two
    // above its parameters explaining itself; the window shows labels and
    // controls now, and what a tool does belongs in the help.
    // The accessible name of a dropdown's trigger, which draws a short label
    // and the value it holds: `In bubbles  MI-GAN` on the bar reads as
    // "Speech bubble text: MI-GAN" to a screen reader. The short form is what
    // is *drawn*; this is what is announced.
    label: {
      choice: '{labelKey}: {value}',
    },
    // What a dropdown's trigger calls its parameter, where the full label is
    // too long to sit on a bar in front of its own value: *Speech bubble text*
    // and *Text outside bubbles* are a sentence each, and the bar shows them
    // side by side with a model name after each. The full label is still the
    // control's accessible name, through `tools.label.choice` above.
    short: {
      bubbleText: 'In bubbles',
      outsideText: 'Outside',
      cleanWith: 'Clean with',
      mode: 'Mode',
      fillMode: 'Fill',
    },
    param: {
      scope: 'Scope',
      bubbleText: 'Speech bubble text',
      outsideText: 'Text outside bubbles',
      // Whether Auto clean touches text outside bubbles at all. The row above
      // names the engine; this one is the opt-in the pipeline design always described
      // and never had a control for.
      outsideBubbles: 'Outside bubbles',
      size: 'Size',
      hardness: 'Hardness',
      spacing: 'Spacing',
      mode: 'Mode',
      color: 'Color',
      // The hex field beside the swatch. It is a second way into the same
      // value rather than a second value, so it takes the row's own label as
      // its accessible name and adds only what makes it different - a
      // colour picker and a text field on one row must not both be called
      // "Color" to a screen reader.
      colorHex: 'Color, hex value',
      // Shown under the hex field while what is in it is not a colour and
      // could not become one - `zzz`, or `#ab` on the way to `#abc`. It says
      // what is wrong and nothing else: the swatch beside it still shows the
      // colour in force, so nothing has been lost and there is nothing to
      // undo.
      colorHexInvalid: 'Not a color: use 3 or 6 hex digits',
      opacity: 'Opacity',
      flow: 'Flow',
      shape: 'Shape',
      feather: 'Feather',
      fillMode: 'Fill mode',
      engine: 'Engine',
      // The AI mask brush's engine row. Deliberately `masks.action.engine`'s
      // words rather than `engine` above: the row on a Layers entry and the
      // chips on the canvas offer the same list of engines under the same
      // names (`src/lib/editor/tools.js#MASK_ENGINES`), and a user who learns
      // one has learned the other.
      cleanWith: 'Clean with',
      alignment: 'Alignment',
    },
    // Controls in the tool bar that are not a parameter of the tool.
    //
    // *Adjustments* is the button that opens the popover holding every slider
    // but Size, and the colour's hex field beside them: a bar has room for the
    // one control a hand reaches for constantly and not for six, and the rest
    // are a press away rather than gone.
    //
    // The eyedropper lives in that popover too, and is drawn only where the
    // platform has `EyeDropper` (Chromium) - absent everywhere else, because
    // the swatch and the hex field are the routes to the same value that every
    // platform has.
    action: {
      adjustments: 'Adjustments',
      eyedropper: 'Pick a color from the screen',
    },
    option: {
      scopePage: 'Page',
      // `outsideBubbles`: hold free text for review, or clean it anyway.
      outsideReview: 'Hold for review',
      outsideClean: 'Clean anyway',
      scopeProject: 'Project',
      // Shapes' `mode` row offers this beside the five cleaning rungs, one of
      // which is called *Fill* - rung 0, the planar fill, which samples the
      // paper around the mask and lays down the tone it found. This one covers
      // the shape in a colour the user picked. Two very different acts, so the
      // word "fill" is spent on the engine and this one says what it is.
      modeSolid: 'Solid colour',
      rect: 'Rect',
      ellipse: 'Ellipse',
      lasso: 'Lasso',
      polygon: 'Polygon',
      engineLocal: 'Local',
      engineCloud: 'Cloud',
      // There is no `engineFill` / `engineRedraw` pair any more. Auto clean's
      // two rows named a *family* - "Fill", "Redraw" - and the user ruled that
      // a picker must name the model; both rows
      // read `masks.engineChoice.*` now, which is the Layers row's own list.
      aligned: 'Aligned',
      nonAligned: 'Non-aligned',
      clone: 'Clone',
      heal: 'Heal',
    },
  },

  /* ================================================================== */
  /* shortcuts - the sheet                                               */
  /* ================================================================== */
  shortcuts: {
    group: {
      tools: 'Tools',
      view: 'View',
      edit: 'Edit',
      zoom: 'Zoom',
      navigation: 'Navigation',
      panels: 'Panels',
      app: 'App',
      // Not one of `SHORTCUT_GROUPS`: the sheet's last section is the binding
      // that is a modifier plus a click rather than a chord, and the shortcut
      // table has no row for it because the table is keys.
      pointer: 'Pointer',
    },
    sheet: {
    },
    // Rebinding. Every refusal names what it refused and what to do instead;
    // `refusedConflict` quotes the shortcut already holding the combination,
    // because "that key is taken" without saying by what is a dead end.
    rebind: {
      change: 'Change the shortcut for {name}',
      reset: 'Reset {name} to its default',
      resetAll: 'Reset all to defaults',
      listening: 'Press a key…',
      unbound: 'Unbound',
      hint: 'Press the combination to use. Escape cancels, Backspace clears the shortcut.',
      fixedHint: 'Escape always cancels, and cannot be changed.',
      refusedAlt:
        'Alt combinations belong to the operating system and are never intercepted here. Choose another combination.',
      refusedKey: 'That key cannot carry a shortcut. Choose another combination.',
      refusedConflict:
        '{nameKey} already uses that combination. Choose another, or change that shortcut first.',
      refusedFixed: 'Escape always cancels, and cannot be rebound or cleared.',
      refusedUnknown: 'This build has no such shortcut.',
    },
    view: {
      holdOriginal: 'Hold to show the original',
      pinOriginal: 'Pin the original',
      maskOverlay: 'Mask overlay',
      reviewFilter: 'Needs-review filter',
    },
    edit: {
      undo: 'Undo',
      redo: 'Redo',
      deleteLayer: 'Delete the selected layer',
    },
    zoom: {
      fit: 'Fit the page',
      in: 'Zoom in',
      out: 'Zoom out',
      actualSize: 'Actual size, 1:1',
    },
    page: {
      left: 'Page left',
      right: 'Page right',
    },
    review: {
      next: 'Next region needing review',
      prev: 'Previous region needing review',
    },
    panel: {
      pages: 'Pages',
      masks: 'Layers & review',
      tools: 'Tool options',
    },
    app: {
      export: 'Export',
      newProject: 'New project',
      openProject: 'Open project',
      settings: 'Settings',
      shortcutSheet: 'This list',
      home: 'Library',
      cancel: 'Cancel or close',
    },
  },

  /* ================================================================== */
  /* time - relative timestamps                                          */
  /* ================================================================== */
  time: {
    relative: {
      justNow: 'just now',
      hoursAgo: {
        one: '1 hour ago',
        other: '{count} hours ago',
      },
      yesterday: 'yesterday',
      daysAgo: {
        one: '1 day ago',
        other: '{count} days ago',
      },
      lastWeek: 'last week',
      weeksAgo: {
        one: '1 week ago',
        other: '{count} weeks ago',
      },
    },
  },

  /* ================================================================== */
  /* notice - the bottom-left queue. Every one of these is a report on    */
  /* something that already happened.                                    */
  /* ================================================================== */
  notice: {
    run: {
      finished: {
        select: 'pages',
        zero: 'Auto clean finished: no pages needed cleaning.',
        one: 'Auto clean finished: 1 page cleaned, {regions} regions.',
        other: 'Auto clean finished: {pages} pages cleaned, {regions} regions.',
      },
      cancelled: 'Auto clean cancelled. The pages already cleaned are kept.',
      nothingInScope: 'Nothing to clean in this scope.',
      // The run could not start because the weights are not on this machine.
      // A notice rather than an error now that there is somewhere to send the
      // reader: before Settings › Models existed, this was a rejected promise
      // the interface had nowhere to put.
      modelsMissing: 'The models are not installed. Download them in Settings › Models.',
      // Pages the run **could not** clean, counted separately from the pages it
      // cleaned, and said even when the number is every page in the chapter.
      // That last case is why this exists: a run where every page failed
      // cleaned no regions, and a zero there used to close the run on
      // `notice.chapter.emptyResult` - "No text found across 0 pages" - which
      // is a dead engine filing a report about the artwork. The pages are left
      // queued rather than marked cleaned, so the sentence says so: nothing has
      // to be undone before running again.
      pagesFailed: {
        select: 'pages',
        one: '1 page could not be cleaned. It is still queued.',
        other: '{pages} pages could not be cleaned. They are still queued.',
      },
      // A model that built, ran, and then stopped answering. Both parameters
      // are keys, resolved before they are interpolated: `models.kind.*` names
      // the model in the words the loaded-models tab uses, and `accel.*` names
      // the provider. Neither is assembled here.
      //
      // The three sentences are three different things the reader needs, in the
      // order they need them: what happened, that it is not permanent, and what
      // to change if it is. On Windows the usual cause is the graphics driver's
      // watchdog resetting the card mid-inference, which poisons the session -
      // so every later run in the same session fails too unless the model is
      // dropped, and that is what "unloaded" is reporting.
      engineFault:
        'The {modelKey} stopped answering on {accelKey}, so it was unloaded. The next run builds it again. If it keeps happening, choose CPU in Settings › Acceleration.',
    },

    // Downloading or replacing the engine runtime itself. Both of these are
    // refusals rather than failures - nothing was half-written - and both name
    // the one thing that would make the press work.
    runtime: {
      // Windows will not let a file that is mapped into a running process be
      // replaced, and the runtime is mapped as soon as anything asks what this
      // machine can accelerate. So the remedy is not "try again": it is a
      // restart, and then the download before anything loads it again.
      inUse:
        'The engine runtime is loaded, and a file in use cannot be replaced. Quit Manga Cleaner, open it again, and download the runtime before you clean anything.',
      // Both figures are byte counts and both are formatted here rather than by
      // whoever counted them, for `models.value.size`'s reason: a size is read,
      // not translated. Saying both is the point - "not enough space" alone
      // leaves the reader guessing how much to clear.
      noSpace:
        'There is not enough free disk space for that download: {needed:memory} needed, {free:memory} free. Free some space and press Download again.',
    },
    chapter: {
      emptyResult: 'No text found across {pages} pages. Nothing to clean.',
      added: 'Ch. {chapter} added to {project}.',
      numberTaken: 'Ch. {chapter} already exists in {project}.',
      deleted: 'Ch. {chapter} deleted. The scans were left where they are.',
      deletedWithScans: 'Ch. {chapter} deleted, and its scans with it.',
      // Asked for and refused: the folder is the project's own, and every other
      // chapter of the project reads it by default.
      deletedProjectFolderKept: 'Ch. {chapter} deleted. Its scans were kept: they are the project’s own folder.',
      deletedSourceUnknown: 'Ch. {chapter} deleted. The library had no record of where its scans are, so none were removed.',
      // Refused rather than created. Two chapters over one folder would hold
      // the same pages under two numbers, and there is no way to correct a
      // chapter's folder afterwards - so the chapter is not made, and the name
      // of the one that already has the folder is what tells the user which
      // folder is meant.
      sourceTaken:
        '“{chapter}” already reads that folder. Choose a folder for this chapter. Two chapters over one folder would hold the same pages twice.',
    },
    convert: {
      finished: 'Converted {count} files to {format}. The originals are kept.',
    },
    project: {
      created: 'Project created. {modeKey} mode is now fixed for every chapter.',
      renamed: 'Renamed to {name}.',
      deleted: '{name} removed from the library. The pages on disk are untouched.',
      sourcePathCopied: 'Source path copied: {path}',
      sourcePathCopyFailed: 'The source path could not be copied to the clipboard.',
      // Not the same as dismissing the chooser, which is a choice and passes in
      // silence. This is the chooser refusing to open, and it says what still
      // works: the field beside the button takes a typed path.
      sourcePickFailed: 'The folder chooser could not be opened. Type the path instead.',
    },
    library: {
      changeFailed: 'That change could not be saved. The library is as it was.',
    },
    history: {
      saveFailed: 'Could not save undo history. Undo may not work after a restart.',
    },
    job: {
      resumed: 'Resuming at page {page}.',
    },
    // The pressure ladder, one notice per step. Each step is
    // recoverable - the regions route down the ladder and the run continues -
    // so these say what the run will do differently rather than that something
    // went wrong.
    memory: {
      // The first step, and it only happens on a run that had rung 3a running -
      // which an automatic run never does. Ahead of
      // the inpainter because it is both the largest resident thing and the
      // only optional one.
      refusedSidecar:
        'This machine is short of memory, so the FLUX sidecar was stopped and will not be started again this run. Regions that needed it fall back to the default inpainter.',
      unloadedInpainter:
        'This machine is short of memory, so the inpainter was unloaded. Regions that needed it are left as they were and listed for review.',
      oneWindow: 'Still short of memory. Down to one page region at a time.',
    },
    input: {
      fileSkipped: {
        one: '1 file skipped: {file} ({reasonKey}).',
        other: '{count} files skipped, starting with {file} ({reasonKey}).',
      },
      junkSkipped: {
        one: '1 junk entry skipped.',
        other: '{count} junk entries skipped.',
      },
      duplicateBasename: '{file} exists with two extensions, so neither was guessed at.',
      // Not a refusal: these pages are in the chapter. The sentence says what
      // was done and to how many, because the editor works on the PNGs from
      // here on and the originals are still where they were.
      converted: {
        one: '1 {from} page was converted to PNG. Your original is untouched.',
        other: '{count} {from} pages were converted to PNG. Your originals are untouched.',
      },
      joinAnomaly: 'Duplicated overlap rows between positions {first} and {second}.',
    },
    cloud: {
      // The whole user-facing content of a privacy guarantee: the request was
      // refused in the interface, before any adapter call, so the page did not
      // leave the machine. It has to say that, not merely that cloud is off.
      blocked: 'Cloud engines are blocked in Settings. Nothing was sent. No part of this page left your machine.',
      returned: 'Cloud region returned in {seconds} s · {cost:currency} billed.',
      rejected: 'Cloud request rejected: {causeKey}. Fell back to the local inpainter.',
      // Not blocked - *absent*. This build has no online engine at all, and the
      // difference matters to the reader: nothing they can change in Settings
      // will make this request work. What it shares with `blocked` is the only
      // part that is a promise: nothing was sent.
      unavailable:
        'This build has no online engine yet. Nothing was sent. No part of this page left your machine.',
    },
    mask: {
      deleted: 'Mask deleted. The original text under it is back.',
      rerunStronger: 'Region re-run one rung stronger: {rungKey}.',
      rerunSimpler: 'Region re-run one rung simpler: {rungKey}.',
      // The two the Layers row's own controls send. `{rungKey}` is the
      // engine's real name here, not the picker's plain-language one: a notice
      // is a record of what ran.
      rerunAgain: 'Region cleaned again: {rungKey}.',
      rerunEngine: 'Region cleaned again with {rungKey}.',
      fillMode: 'Fill mode is now {fillModeKey}.',
      reopened: 'Mask kept, reopened in {toolKey}.',
      // What a region edit says when no engine would produce a patch for it.
      // The region is left exactly as it was - the same rule
      // holds for a hand edit as much as for a run - so the sentence has
      // to say that nothing changed, or a user reads silence as success.
      rerunFailed: 'That area could not be cleaned: {reasonKey}. It is exactly as it was.',
    },
    tool: {
      cloneSourceSet: 'Clone source sampled. Paint to copy from it.',
      cloneNeedsSource: 'Alt-click a source first. Clone and heal copy from somewhere.',
    },
    gate: {
      cleanedAnyway: 'Cleaned despite the script gate.',
    },
    export: {
      finished: 'Exported {count} pages as {format} to {path}.',
      // The stitched sentence carries the gutter count, because a file that
      // invented pixels should say how many on the way past.
      stitched: {
        zero: 'Exported the chapter as one {format} file to {path}. No pixel was invented: every page is the full width.',
        one: 'Exported the chapter as one {format} file to {path}. 1 pixel of paper white was written beside a narrower page.',
        other:
          'Exported the chapter as one {format} file to {path}. {count} pixels of paper white were written beside the narrower pages.',
      },
      refusedOverwrite: 'Export refused: it would replace the files in {path}. Nothing was written.',
      // The seven refusals that used to be silence or a substituted format.
      // Each says what was asked for, what happened instead - nothing - and
      // what would work, because a refusal with no way out of it is a dead end
      // dressed as an explanation.
      refusedLossyFormat:
        '{format} is a lossy format, and this build has no way to tell you what it would cost your pages before it wrote them. Nothing was written. Export PNG or TIFF, which keep every pixel.',
      refusedLayeredFormat:
        '{format} is not written by this build. Nothing was written. Export PSD, which carries the same layers for pages up to 30 000 pixels a side.',
      refusedUnknownFormat:
        'This build cannot write {format}. Nothing was written. The formats it can write are PNG, TIFF, PSD and CBZ.',
      refusedMaskLayers:
        'A CBZ is read page by page, so a mask file inside it would show as a page. Nothing was written. Export PNG or TIFF for a mask file beside each page, or PSD for a layer mask on each region.',
      refusedStitchedLayered:
        'A PSD holds one whole page as its Background, and this build never holds the whole chapter as one image. Nothing was written. Export PSD per page, or one PNG or TIFF for the chapter.',
      refusedLayeredMode:
        'PSD cannot carry an indexed-colour page or one below 8 bits, and converting would change pixels you did not ask to change. Nothing was written. Export PNG, which carries them as they are.',
      refusedLayeredSize:
        'A page here is over 30 000 pixels on a side, which is more than a PSD can hold. Nothing was written. Export PNG or TIFF, which have no such limit.',
      refusedDestination:
        'That destination is not somewhere this export can write. Nothing was written. Choose a new folder, or give a full path from the top of the disk.',
      refusedStitchPaginated:
        'One file for the chapter is a longstrip layout, and this project is paginated. Nothing was written. Its pages are separate images, and stacking them would make a document that has no original.',
      refusedStitchedArchive:
        'A CBZ holds one file per page, so it cannot hold one file for the chapter. Nothing was written. Export the chapter as a single PNG or TIFF, or keep the archive and export per page.',
      // One per `StitchRefusal`, and each names the condition rather than the
      // page: `reasonKey` is the whole of what crosses the seam, so a sentence
      // naming a page number would be naming one nothing carried.
      stitchRefused: {
        noPages: 'There are no pages to stitch into one file. Nothing was written.',
        mixedMode:
          'These pages are not all in the same colour mode, and one file has one colour mode. Nothing was written. Converting them would change pixels you did not ask to change. Export per page instead.',
        mixedDepth:
          'These pages are not all the same bit depth, and one file has one bit depth. Nothing was written. Export per page instead.',
        mixedProfile:
          'These pages do not all carry the same colour profile, and one file has one profile. Nothing was written. Export per page instead.',
        indexed:
          'These pages use indexed colour, and one file would need one palette across all of them. Nothing was written. Reconciling the palettes would shift colours, so export per page instead.',
      },
    },
  },

  /* ================================================================== */
  /* update - in-app auto-updater                                       */
  /* ================================================================== */
  update: {
    title: 'Update available',
    field: {
      version: 'Version',
      releaseNotes: 'Release notes',
    },
    notes: {
      empty: 'No release notes.',
    },
    status: {
      downloading: 'Downloading update…',
      upToDate: 'Manga Cleaner is up to date.',
      checkFailed: 'Could not check for updates.',
    },
    action: {
      check: 'Check for updates',
      checking: 'Checking for updates…',
      download: 'Download & install',
      downloading: 'Downloading…',
      downloadingPercent: 'Downloading {percent}%…',
      restart: 'Restart Manga Cleaner',
      later: 'Later',
      available: 'Update available',
      availableTag: 'Update available: {version}',
      updateTo: 'Update to v{version}',
    },
  },
}
