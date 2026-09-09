/**
 * What the tool bar shows for each tool: its name, the hint it carries as a
 * tooltip, and the parameters themselves.
 *
 * **No prose.** The bar carries controls and nothing that explains them: the
 * tools are for translators rather than for retouchers, and a paragraph over
 * every tool is a paragraph nobody reads twice. What a tool does belongs in the
 * help, which is one place rather than six.
 *
 * Every number here is the design file's (`defs` in `renderVals()`). Nothing in
 * this module is user-visible text - labels are i18n keys, and the values are
 * the ids `src/lib/state/editor.svelte.js` stores in `editor.toolParams`.
 *
 * The tool ids and their `1`–`6` order come from `TOOLS` in the state module;
 * this file must stay in step with it, which `toolSpec()` asserts by returning
 * the Auto clean spec for anything it does not recognise.
 */

import { ROW_ENGINES, engineChoiceLabel } from '../model/masks.js'

/**
 * @typedef {Object} RangeParam
 * @property {'range'} kind
 * @property {string} key - the field in `editor.toolParams[tool]`
 * @property {string} labelKey
 * @property {number} min
 * @property {number} max
 * @property {number} step
 * @property {'px'|'%'} [unit]
 * @property {string} [group] - the group this parameter belongs to
 * @property {(values: Record<string, unknown>) => boolean} [when] - live only while this holds
 */

/**
 * @typedef {Object} ChoiceOption
 * @property {string} value
 * @property {string} labelKey
 * @property {string} [icon] - a glyph from `src/lib/icons/paths.js`. Where
 *   *every* option of a parameter carries one the bar draws the parameter as a
 *   group of icon cells and no words; where none does, it is a dropdown. A
 *   parameter is one or the other, never a mixture.
 * @property {boolean} [cloud]
 * @property {boolean} [sidecar]
 * @property {string} [engine]
 */

/**
 * @typedef {Object} ChoiceParam
 * @property {'choice'} kind
 * @property {string} key
 * @property {string} labelKey
 * @property {string} [shortKey] - what the dropdown's trigger calls this
 *   parameter, where the full label is too long to sit on a bar in front of its
 *   own value. The full label is still the control's accessible name.
 * @property {ChoiceOption[]} options
 * @property {string} [group]
 * @property {(values: Record<string, unknown>) => boolean} [when]
 */

/**
 * @typedef {Object} ColorParam
 * @property {'color'} kind
 * @property {string} key
 * @property {string} labelKey
 * @property {string} [default]
 * @property {string} [group]
 * @property {(values: Record<string, unknown>) => boolean} [when]
 */

/**
 * @typedef {Object} ToolSpec
 * @property {string} id
 * @property {number} slot - the number key that selects it
 * @property {string} nameKey
 * @property {string} hintKey - the bar's tooltip on the tool's own name
 * @property {Array<RangeParam|ChoiceParam|ColorParam>} params
 * @property {boolean} runnable - carries the run / cancel action button
 */

/**
 * A parameter that is only *live* under some other parameter's value.
 *
 * The colour a Shape is filled with means nothing while the Shape is being
 * cleaned by an engine rather than painted flat. A control that cannot change what happens is
 * worse than no control (the same rule that removed the AI mask brush's three
 * dead rows), so those rows are **absent** rather than disabled: nothing is
 * blocked and there is no reason to explain.
 *
 * The value is still kept in `editor.toolParams` while the row is away, so
 * switching back to the mode restores the colour that was picked for it.
 *
 * @template {RangeParam|ChoiceParam|ColorParam} P
 * @param {P} param
 * @param {(values: Record<string, unknown>) => boolean} predicate
 * @returns {P}
 */
function onlyWhen(param, predicate) {
  return { ...param, when: predicate }
}

/**
 * The parameters of a spec that are live for these values - what the tool bar
 * renders, and the one place the `when` predicates are read.
 *
 * @param {ToolSpec} spec
 * @param {Record<string, unknown>} values
 * @returns {Array<RangeParam|ChoiceParam|ColorParam>}
 */
export function activeParams(spec, values = {}) {
  return spec.params.filter(
    (param) => typeof (/** @type {any} */ (param).when) !== 'function' ||
      /** @type {any} */ (param).when(values ?? {}),
  )
}

/**
 * Put a run of parameters in one group.
 *
 * The group is the bar's only structure, and it is what the hairlines separate:
 * "Speech bubble text" and "Text outside bubbles" are the two halves of one
 * question and stand together, away from the scope beside them. Grouping lives
 * here rather than in the component because it is a statement about the
 * parameters - which of them are the same decision - and the component's job is
 * to draw whatever it is handed.
 *
 * The id is a **plain name and not an i18n key**. It was a key while the tool
 * window drew a small uppercase heading over each group; a bar has no second
 * line to put a heading on, and a group that is drawn as a hairline and read
 * out as nothing does not need a word.
 *
 * The order inside a group is the order it is written in, and the groups
 * themselves are read in spec order by `paramGroups`, so a group is a **run of
 * adjacent parameters** and never a filter that reshuffles them.
 *
 * @template {RangeParam|ChoiceParam|ColorParam} P
 * @param {string} groupKey
 * @param {P[]} params
 * @returns {P[]}
 */
function section(groupKey, params) {
  return params.map((param) => ({ ...param, group: groupKey }))
}

/**
 * The live rows of a spec, split into the sections the tool bar draws.
 *
 * A group ends where the next row names a different one, so a row hidden by
 * its `when` predicate takes no group with it - Shapes in an engine mode loses
 * its colour and its opacity and the *Mode* group stays, because `mode` and
 * `feather` are still in it.
 *
 * @param {ToolSpec} spec
 * @param {Record<string, unknown>} [values]
 * @returns {Array<{key: string|null, params: Array<RangeParam|ChoiceParam|ColorParam>}>}
 */
export function paramGroups(spec, values = {}) {
  /** @type {Array<{key: string|null, params: Array<RangeParam|ChoiceParam|ColorParam>}>} */
  const groups = []
  for (const param of activeParams(spec, values)) {
    const key = /** @type {any} */ (param).group ?? null
    const last = groups[groups.length - 1]
    if (last && last.key === key && key !== null) last.params.push(param)
    else groups.push({ key, params: [param] })
  }
  return groups
}

/**
 * @param {string} key
 * @param {string} labelKey
 * @param {number} min
 * @param {number} max
 * @param {number} step
 * @param {'px'|'%'} [unit]
 * @returns {RangeParam}
 */
function range(key, labelKey, min, max, step, unit) {
  return { kind: 'range', key, labelKey, min, max, step, unit }
}

/**
 * @param {string} key
 * @param {string} labelKey
 * @param {ChoiceOption[]} options
 * @param {string} [shortKey] - the trigger's own label, where the row is a
 *   dropdown and the full one is too long for a bar
 * @returns {ChoiceParam}
 */
function choice(key, labelKey, options, shortKey) {
  return shortKey
    ? { kind: 'choice', key, labelKey, shortKey, options }
    : { kind: 'choice', key, labelKey, options }
}

/**
 * @param {string} key
 * @param {string} labelKey
 * @param {string} [defaultValue]
 * @returns {ColorParam}
 */
export function color(key, labelKey, defaultValue = '#000000') {
  return { kind: 'color', key, labelKey, default: defaultValue }
}

/**
 * Which **model** a kind of text starts on, offered once per kind of text.
 *
 * These used to be two words - `fill` and `redraw` - deliberately named for
 * what the user watches happen rather than for the rung that does it. The user
 * ruled against that: an engine picker exists
 * so that somebody can move between models, and two verbs that each stand for
 * a family of two make that impossible. So the row is the ladder's own list,
 * under the ladder's own names - the same list and the same words a Layers
 * row's picker offers (`ROW_ENGINES`, `engineChoiceLabel`).
 *
 * **Rung 3a is not on it**, and that is not a naming decision: `runClean` has
 * no confirmation protocol, so `HIGHEST_AUTOMATIC` in `src-tauri/src/run.rs`
 * caps every automatic run at manga-LaMa whatever it is asked for. A FLUX
 * entry here would be a control that silently did nothing on every region of
 * every page. It stays reachable per region,
 * where a person chooses it and waits for it.
 *
 * A choice here is a **starting point, not a ceiling**: `clean_region` in
 * `src-tauri/src/run.rs` still escalates when the quality metric declines a
 * patch, so picking `fill` for a bubble that turns out to be screentone still
 * ends on the inpainter rather than on a hole.
 */
const ENGINES = ROW_ENGINES.filter((rung) => rung !== 'flux').map((rung) => ({
  value: rung,
  labelKey: engineChoiceLabel(rung),
  // Which rung this option *is*, so `ToolBar#optionsFor` can drop the ones
  // whose weights are not on this machine. Written here rather than inferred
  // from `value` because `value` is a tool parameter and could stop being a
  // rung name; the gate is about the ladder either way.
  engine: rung,
}))

/**
 * The engines the AI mask brush may be pointed at, weakest first.
 *
 * **The Layers row's own list and the Layers row's own words** - `ROW_ENGINES`
 * and `engineChoiceLabel` from `src/lib/model/masks.js`, not a second list
 * beside them. The stroke and the row are asking the same question of the same
 * region ("clean this with *that*"), and a user who learns "LaMa" on a row
 * must not meet a different vocabulary for it on the canvas.
 *
 * Unlike `ENGINES` above these are **rungs named outright, not picks**: what
 * `src-tauri/src/region.rs#named_rung` reads from `params.engine`, and it runs
 * that rung rather than starting there. This row offers all four rungs; Auto
 * clean's two offer the three local ones, because `ENGINES` above drops `flux`
 * from a run nobody is watching. The difference between naming a rung and
 * picking one is what a run does after the first patch - Auto clean escalates
 * past a declined rung, a named one is committed as it came out.
 *
 * **A rung is offered only where it can run.** `flux` needs the sidecar and
 * rung 2 needs weights that are downloaded after install, so every option
 * carries the rung it is and `ToolBar.svelte` drops the ones
 * `state/capabilities.svelte.js` reports missing - hidden rather than
 * disabled, and with the sidecar's own reason rendered underneath where there
 * is anything to say.
 */
const MASK_ENGINES = ROW_ENGINES.map((rung) => ({
  value: rung,
  labelKey: engineChoiceLabel(rung),
  engine: rung,
  // Rung 3a alone, and it carries a *second* flag because it is the one rung
  // with something to say when it is missing: `capabilities.sidecarReasonKey`
  // is a sentence about a sidecar that is installed and unusable, which no
  // undownloaded weight has an equivalent of. `ToolBar#gateNote` renders it.
  sidecar: rung === 'flux',
}))

/**
 * The value of Shapes' `mode` that means **paint this shape flat**, rather than
 * clean what is under it.
 *
 * It is not called `fill`, and the distance between the two words is the whole
 * reason it has a name at all: `fill` is rung 0, the *planar fill* engine,
 * which samples the paper around the mask and lays down the tone it found -
 * and it is one of the four engine options beside this one. `solid` covers the shape
 * in the colour the user picked, which is a different act with a different
 * outcome. The label says "Solid colour" for the same reason.
 */
export const SOLID = 'solid'

/**
 * @param {Record<string, unknown>} values
 * @returns {boolean} whether Shapes is set to paint rather than to clean
 */
export function isSolidFill(values) {
  return (values ?? {}).mode === SOLID
}

/**
 * What a drawn shape is *for*: a flat colour, or one of the cleaning engines.
 *
 * One row rather than two, because the two are alternatives rather than
 * settings of one another - a shape is either paint or a clean, never both -
 * and a second row would be a control that is dead half the time. The engines
 * are `MASK_ENGINES` unchanged, so Shapes, the AI mask brush and a Layers
 * row's picker all offer the same four rungs under the same four words, and
 * `ToolBar.svelte` gates rung 3a here exactly as it does there.
 */
const SHAPE_MODES = [{ value: SOLID, labelKey: 'tools.option.modeSolid' }, ...MASK_ENGINES]

/** @type {ToolSpec[]} */
export const TOOL_SPECS = [
  {
    id: 'autoClean',
    slot: 1,
    nameKey: 'tools.name.autoClean',
    hintKey: 'tools.hint.autoClean',
    params: [
      ...section('scope', [
        choice('scope', 'tools.param.scope', [
          { value: 'page', labelKey: 'tools.option.scopePage', icon: 'file' },
          { value: 'project', labelKey: 'tools.option.scopeProject', icon: 'book' },
        ]),
      ]),
      // Two rows rather than one, because the two kinds of text want opposite
      // engines and always did: a speech balloon is flat white or flat black
      // and the fill family covers it for nothing, where text over art has to
      // have the art put back. The run used to route both by the fit alone,
      // which sent bubble text to the inpainter whenever the fit was unsettled
      // - a 510 MB session and a second a region to redraw paper that a fill
      // would have matched exactly. `bubbleEngine` and `outsideEngine` travel
      // to `runClean` and are applied per region by whether the region is
      // inside a balloon.
      ...section('engines', [
        choice('bubbleEngine', 'tools.param.bubbleText', ENGINES, 'tools.short.bubbleText'),
        choice('outsideEngine', 'tools.param.outsideText', ENGINES, 'tools.short.outsideText'),
        // The pipeline's own rule: text outside a balloon is "cleaned only if
        // the user opts in". This is the opt-in. Off, the run
        // holds that text for review under "text outside a speech bubble" and
        // the row above only bites through *Clean anyway*; on, every such
        // region goes to the ladder starting on the row above's rung. No
        // script is read for it either way - sound effects are conceded, not
        // classified - so the person turning this on is the one deciding the
        // chapter's free text is all safe to clean.
        choice('outsideBubbles', 'tools.param.outsideBubbles', [
          { value: 'review', labelKey: 'tools.option.outsideReview' },
          { value: 'clean', labelKey: 'tools.option.outsideClean' },
        ]),
      ]),
      // There is deliberately no engine-ceiling row. It offered two rungs of
      // src/lib/model/ladder.js - the highest local engine, or the whole ladder
      // including cloud - and the cloud rung was a way to spend money that
      // never met the disclosure: `needs-confirmation` lives on `applyTool`,
      // and `runClean` has no equivalent, so a run at that ceiling sent pages
      // off the machine with neither the transmission statement nor the cost
      // confirmation. Rather than grow the run protocol, the user ruled that
      // a batch run is local-only:
      // an unbounded batch spend is the hardest kind to confirm meaningfully,
      // and cloud stays reachable per-region through Content-aware fill, which
      // carries the whole flow. `startRun` pins the ceiling to LOCAL_CEILING,
      // so this is enforced where the sending happens, not merely unoffered
      // here. With the cloud rung gone the row held one option, and a
      // radiogroup of one is worse than no row at all.
    ],
    runnable: true,
  },
  {
    id: 'brush',
    slot: 2,
    nameKey: 'tools.name.brush',
    hintKey: 'tools.hint.brush',
    params: [
      ...section('brush', [
        range('size', 'tools.param.size', 4, 120, 2, 'px'),
        range('hardness', 'tools.param.hardness', 0, 100, 5, '%'),
        range('spacing', 'tools.param.spacing', 1, 50, 1, '%'),
      ]),
      // **The brush paints, and that is all it does.** It used to carry a
      // three-way `mode` row - add to a mask, erase from one, or paint - and
      // the first two were a second, worse route to work the rest of the
      // editor already owns: the AI mask brush strokes a mask an engine then
      // cleans, and a mask is removed from the Layers row that lists it or
      // from the region menu over it. What was left was a row whose two dead
      // options hid the colour, opacity and flow behind a chip press. The
      // parameters are unconditional now, and `mode` stays pinned to `paint`
      // in `editor.toolParams` because it is what the seam reads
      // (`paint.js#paintParamsOf`, `region.rs#paint_plan`).
      ...section('paint', [
        color('color', 'tools.param.color'),
        range('opacity', 'tools.param.opacity', 0, 100, 5, '%'),
        range('flow', 'tools.param.flow', 0, 100, 5, '%'),
      ]),
    ],
    runnable: false,
  },
  {
    id: 'shapes',
    slot: 3,
    nameKey: 'tools.name.shapes',
    hintKey: 'tools.hint.shapes',
    params: [
      ...section('shape', [
        choice('shape', 'tools.param.shape', [
          { value: 'rect', labelKey: 'tools.option.rect', icon: 'shape-rect' },
          { value: 'ellipse', labelKey: 'tools.option.ellipse', icon: 'shape-ellipse' },
          { value: 'lasso', labelKey: 'tools.option.lasso', icon: 'shape-lasso' },
          { value: 'polygon', labelKey: 'tools.option.polygon', icon: 'shape-polygon' },
        ]),
      ]),
      // What the shape *does*. Shapes used to have no such row and always
      // cleaned with whatever the fill mode implied, so the one tool whose
      // gesture describes an area outright was the one tool that could not be
      // pointed at an engine - and could not lay down a colour at all, which
      // is what a shape is reached for when a credit block or a stray panel
      // gutter has to go flat.
      ...section('mode', [
        choice('mode', 'tools.param.mode', SHAPE_MODES, 'tools.short.mode'),
        onlyWhen(color('color', 'tools.param.color'), isSolidFill),
        // The paint plan carries a stroke-level opacity, so a solid fill can be
        // a wash as well as a cover. Meaningless for the engine modes, which
        // replace what is under them outright.
        onlyWhen(range('opacity', 'tools.param.opacity', 0, 100, 5, '%'), isSolidFill),
        range('feather', 'tools.param.feather', 0, 20, 1, 'px'),
      ]),
    ],
    runnable: false,
  },
  {
    id: 'aiMaskBrush',
    slot: 4,
    nameKey: 'tools.name.aiMaskBrush',
    hintKey: 'tools.hint.aiMaskBrush',
    params: [
      // Which engine, then how wide the stroke is. The order is the order the
      // gesture is thought about: pick what should happen to the paint, set the
      // brush, then paint.
      //
      // **There used to be three more** - snap strength, show confidence,
      // fallback on/off - and all three described a snapping mechanism that no
      // longer exists. The stroke is the mask now, so there is nothing to snap
      // to, no confidence to show and nothing to fall back from. A control
      // that cannot change what happens is worse than no control.
      ...section('engines', [
        choice('engine', 'tools.param.cleanWith', MASK_ENGINES, 'tools.short.cleanWith'),
      ]),
      ...section('brush', [range('size', 'tools.param.size', 8, 160, 4, 'px')]),
    ],
    runnable: false,
  },
  {
    id: 'contentAwareFill',
    slot: 5,
    nameKey: 'tools.name.contentAwareFill',
    hintKey: 'tools.hint.contentAwareFill',
    params: [
      ...section('fill', [
        choice(
          'fillMode',
          'tools.param.fillMode',
          [
            { value: 'match-surround', labelKey: 'masks.fillMode.matchSurround' },
            { value: 'reconstruct', labelKey: 'masks.fillMode.reconstruct' },
            { value: 'solid', labelKey: 'masks.fillMode.solid' },
          ],
          'tools.short.fillMode',
        ),
        choice('engine', 'tools.param.engine', [
          { value: 'local', labelKey: 'tools.option.engineLocal', icon: 'cpu' },
          { value: 'cloud', labelKey: 'tools.option.engineCloud', cloud: true, icon: 'cloud' },
        ]),
      ]),
    ],
    runnable: false,
  },
  {
    id: 'cloneHeal',
    slot: 6,
    nameKey: 'tools.name.cloneHeal',
    hintKey: 'tools.hint.cloneHeal',
    params: [
      ...section('brush', [
        range('size', 'tools.param.size', 4, 120, 2, 'px'),
        range('hardness', 'tools.param.hardness', 0, 100, 5, '%'),
        range('opacity', 'tools.param.opacity', 0, 100, 5, '%'),
        range('flow', 'tools.param.flow', 0, 100, 5, '%'),
      ]),
      ...section('mode', [
        choice('alignment', 'tools.param.alignment', [
          { value: 'aligned', labelKey: 'tools.option.aligned', icon: 'link' },
          { value: 'nonAligned', labelKey: 'tools.option.nonAligned', icon: 'link-off' },
        ]),
        choice('mode', 'tools.param.mode', [
          { value: 'clone', labelKey: 'tools.option.clone', icon: 'stamp' },
          { value: 'heal', labelKey: 'tools.option.heal', icon: 'bandage' },
        ]),
      ]),
    ],
    runnable: false,
  },
]

/** The rail's icon per tool, in `TOOL_SPECS` order. */
export const TOOL_ICONS = /** @type {Record<string, string>} */ ({
  autoClean: 'sparkle',
  brush: 'brush',
  shapes: 'shapes',
  aiMaskBrush: 'wand',
  contentAwareFill: 'droplet',
  cloneHeal: 'stamp',
})

/**
 * @param {string} id
 * @returns {ToolSpec} the Auto clean spec for an unknown id - the tool bar
 *   always has something to show, and the mismatch is visible rather than blank
 */
export function toolSpec(id) {
  return TOOL_SPECS.find((spec) => spec.id === id) ?? TOOL_SPECS[0]
}

/**
 * The tools whose gesture on the page is a **drag**, and which therefore need
 * a drawing surface over the sheet.
 *
 * Auto clean and Content-aware fill are not here on purpose: Auto clean runs a
 * queue and Content-aware fill fills an *existing* mask, so both act on a
 * region that is already there and both are a click on it. The
 * drawing surface would only take that click away from them.
 */
export const DRAWING_TOOLS = /** @type {const} */ ([
  'brush',
  'shapes',
  'aiMaskBrush',
  'cloneHeal',
])

/**
 * @param {string} id
 * @returns {boolean}
 */
export function isDrawingTool(id) {
  return DRAWING_TOOLS.includes(/** @type {any} */ (id))
}

/**
 * Would running this tool with these parameters send anything to the cloud?
 *
 * Derived from the specs' own `cloud` flags rather than from a second list of
 * tool-and-parameter names: the option that is disabled in the tool bar when
 * `session.cloudAllowed` is false is exactly the option that must not be sent,
 * and one source for both is what keeps them from drifting apart.
 *
 * @param {string} id
 * @param {Record<string, unknown>} [params]
 * @returns {boolean}
 */
export function toolSpendsCloud(id, params = {}) {
  const spec = TOOL_SPECS.find((candidate) => candidate.id === id)
  if (!spec) return false
  return spec.params.some(
    (param) =>
      param.kind === 'choice' &&
      param.options.some((option) => option.cloud && option.value === params[param.key]),
  )
}

/**
 * Compute the roving tab stop index for a segmented control.
 *
 * The selected option holds the tab stop only if it is enabled. When nothing is
 * selected or the selected option is disabled, the first enabled option holds the
 * tab stop so the control remains reachable via Tab.
 *
 * @param {Array<{disabled?: boolean}>} items
 * @param {number} selected
 * @returns {number}
 */
export function segmentedTabStop(items, selected) {
  if (selected >= 0 && !items[selected]?.disabled) return selected
  return items.findIndex((o) => !o.disabled)
}

/**
 * Compute the effective value for a choice parameter given available options.
 *
 * When an engine or mode is filtered out (e.g. after a sidecar or model download
 * is removed), the stored value may point to an option that no longer exists in
 * the UI. In that case, fallback to the first available option. If the stored
 * value is still available, keep it. If no options are available, keep the stored
 * value (or parameter default).
 *
 * @param {ChoiceParam} [param]
 * @param {Array<string | {value: string}>} [availableOptions]
 * @param {unknown} [storedValue]
 * @returns {string}
 */
export function effectiveChoice(param, availableOptions = [], storedValue) {
  const fallback = param?.options?.[0]?.value ?? ''
  const current = storedValue !== undefined && storedValue !== null ? String(storedValue) : fallback
  if (!availableOptions || availableOptions.length === 0) {
    return current
  }
  const match = availableOptions.find((opt) =>
    typeof opt === 'string' ? opt === current : opt?.value === current,
  )
  if (match) {
    return typeof match === 'string' ? match : match.value
  }
  const first = availableOptions[0]
  return typeof first === 'string' ? first : (first?.value ?? current)
}

/**
 * Validate and format hex color text during user input.
 *
 * Commits only when the text is exactly 6 hex digits (with optional leading #).
 * Partial inputs and 3-digit shorthands return null while typing so the user can
 * finish typing 6-digit hex values without premature expansion or overwrites.
 *
 * @param {string} text
 * @returns {string|null}
 */
export function hexOnInput(text) {
  const raw = String(text ?? '').trim().replace(/^#/, '')
  return /^[0-9a-f]{6}$/i.test(raw) ? `#${raw.toLowerCase()}` : null
}

/**
 * Validate and format hex color text when the input commits (blur or Enter).
 *
 * Accepts 6-digit hex colors as well as 3-digit shorthand (expanding #abc to
 * #aabbcc). Incomplete or invalid text returns null.
 *
 * @param {string} text
 * @returns {string|null}
 */
export function hexOnCommit(text) {
  const raw = String(text ?? '').trim().replace(/^#/, '')
  if (/^[0-9a-f]{6}$/i.test(raw)) {
    return `#${raw.toLowerCase()}`
  }
  if (/^[0-9a-f]{3}$/i.test(raw)) {
    return `#${[...raw].map((digit) => digit + digit).join('').toLowerCase()}`
  }
  return null
}

/**
 * Is the text in the hex field something that will never become a colour?
 *
 * The field used to commit silently or not at all: `zzz` and `#ab` both wrote
 * nothing and both looked exactly like a value that had been accepted. This
 * is the question the row asks to decide
 * whether to say so - `aria-invalid` on the field and a muted line under it.
 *
 * **A half-typed value is not wrong.** `#ab` on the way to `#abc` answers
 * true, because it is not a colour *yet* and the row's note is what tells the
 * user the swatch has not moved; the note goes as soon as the third digit
 * lands. An **empty** field is not wrong either way: nothing has been typed,
 * so there is nothing to reject, and the swatch still shows the colour that is
 * in force.
 *
 * @param {string} text
 * @returns {boolean}
 */
export function hexInvalid(text) {
  const raw = String(text ?? '').trim()
  if (raw === '') return false
  return hexOnCommit(raw) === null
}
