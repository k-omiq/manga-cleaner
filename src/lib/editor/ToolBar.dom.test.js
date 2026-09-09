/**
 * The tool bar, mounted.
 *
 * `tools.test.js` asserts the *specs* - every parameter is in a group, the
 * order is unchanged, a hidden one takes no group with it, and which choices
 * carry icons - and the rendering of them is this file's: which control a
 * parameter draws, whether it is on the bar or behind the Adjustments button,
 * and whether pressing it writes the parameter.
 *
 * The `.dom` in the name is the whole of the environment change. The repo's
 * vitest runs `environment: 'node'` and stays there: a DOM per test file costs
 * a second of start-up, and
 * every other test in `src/lib` is arithmetic that does not need one. This file
 * asks for a document - and for Svelte's browser build, which is the half a
 * pragma cannot ask for - because the thing under test *is* the document.
 *
 * The three stores are stubbed rather than driven. `session`, `capabilities`
 * and `editor` are the bar's whole input, and a stub of each is what lets a
 * spec be mounted with every engine available and nothing blocked - which is
 * the case where the control a parameter draws is decided by the parameter and
 * not by a missing download.
 */

import { describe, expect, it, afterEach, vi } from 'vitest'
import { render, cleanup, fireEvent } from '@testing-library/svelte'
import { tick } from 'svelte'
import { TOOL_SPECS, activeParams, toolSpec } from './tools.js'
import { t } from '../i18n/index.js'

/**
 * The stubs, hoisted so `vi.mock`'s factories can close over them.
 *
 * `session` and `capabilities` are plain objects: nothing in them mutates
 * while a component is mounted. `editor` is the **real** store - the module's
 * own `$state` proxy, with its functions replaced by spies - because the tests
 * that switch tool under a mounted bar need the switch to re-render it, and a
 * plain object would change without the bar noticing.
 */
const { editor } = await vi.importActual('../state/editor.svelte.js')

const stores = vi.hoisted(() => ({
  session: {
    cloudAllowed: true,
    /** @type {Record<string, {x: number, y: number, w: number, h: number|null, open: boolean, fold: boolean}>} */
    windows: { tool: { x: 16, y: 62, w: 560, h: null, open: true, fold: false } },
    stacking: { tool: 1 },
  },
  capabilities: {
    /** @type {Record<string, boolean>} */
    engines: {},
    sidecar: true,
    /** @type {string|null} */
    sidecarReasonKey: null,
    runtime: true,
    autoClean: true,
  },
  setToolParam: vi.fn(),
  startRun: vi.fn(),
  cancelRun: vi.fn(),
  setWindowBox: vi.fn(),
  setWindowOpen: vi.fn(),
  commitWindows: vi.fn(),
}))

stores.editor = editor

vi.mock('../state/editor.svelte.js', () => ({
  editor,
  setToolParam: stores.setToolParam,
  startRun: stores.startRun,
  cancelRun: stores.cancelRun,
  currentPage: () => ({ id: 'p1', number: 1 }),
  runningPage: () => null,
}))

vi.mock('../state/session.svelte.js', () => ({
  session: stores.session,
  setWindowBox: stores.setWindowBox,
  commitWindows: stores.commitWindows,
  raiseWindow: vi.fn(),
  foldWindow: vi.fn(),
  setWindowOpen: stores.setWindowOpen,
  resetWindowBox: vi.fn(),
}))

vi.mock('../state/capabilities.svelte.js', () => ({ capabilities: stores.capabilities }))

// Imported after the mocks so the component picks the stubs up.
const { default: ToolBar } = await import('./ToolBar.svelte')

/**
 * The values a spec starts on: what the bar would read out of
 * `editor.toolParams` on a fresh install. Written from the spec rather than
 * copied, so a parameter added to a tool is covered by this file without it
 * being edited - and so the bar's own reconciling effect finds nothing to
 * correct and calls the setter zero times before a test does.
 *
 * @param {import('./tools.js').ToolSpec} spec
 * @returns {Record<string, unknown>}
 */
function startingValues(spec) {
  /** @type {Record<string, unknown>} */
  const values = {}
  for (const param of spec.params) {
    if (param.kind === 'range') values[param.key] = param.min
    else if (param.kind === 'choice') values[param.key] = param.options[0].value
    else values[param.key] = param.default ?? '#000000'
  }
  return values
}

/**
 * Mount the bar with a tool selected.
 *
 * @param {import('./tools.js').ToolSpec} spec
 * @param {Record<string, unknown>} [overrides] - parameters this mount starts on
 *   instead of the spec's own opening values
 */
function mountTool(spec, overrides) {
  stores.editor.tool = spec.id
  stores.editor.toolParams = { [spec.id]: { ...startingValues(spec), ...overrides } }
  const view = render(ToolBar)
  stores.setToolParam.mockClear()
  return { view, values: stores.editor.toolParams[spec.id] }
}

/** A choice the bar draws as icon cells rather than as a dropdown. */
const iconChoice = (param) => param.options.every((option) => option.icon)

/** Everything but `size` lives behind the Adjustments button, and so does the hex field. */
const onBar = (param) => param.kind !== 'range' || param.key === 'size'

/** @param {import('@testing-library/svelte').RenderResult} view */
function openAdjustments(view) {
  const button = view.getByLabelText(t('tools.action.adjustments'))
  return fireEvent.click(button)
}

/**
 * The element a parameter draws, wherever it drew it. The accessible name is
 * what finds it - which is also an assertion that every control has one.
 *
 * @param {import('@testing-library/svelte').RenderResult} view
 * @param {any} param
 * @param {Record<string, unknown>} values
 */
function controlFor(view, param, values) {
  if (param.kind === 'range' || param.kind === 'color') {
    return view.getByLabelText(t(param.labelKey))
  }
  if (iconChoice(param)) return view.getByLabelText(t(param.labelKey))
  // A dropdown's trigger is named for the parameter *and* the value it holds.
  const current = param.options.find((option) => option.value === values[param.key])
  return view.getByLabelText(
    t('tools.label.choice', { labelKey: param.labelKey, value: t(current.labelKey) }),
  )
}

afterEach(() => {
  cleanup()
  vi.clearAllMocks()
})

describe('every tool draws the control each of its parameters asks for', () => {
  for (const spec of TOOL_SPECS) {
    it(`draws ${spec.id}'s parameters, each with a name of its own`, async () => {
      const { view, values } = mountTool(spec)
      const params = activeParams(spec, values)
      // Nothing is behind a press until the press is made.
      if (params.some((param) => !onBar(param))) await openAdjustments(view)

      for (const param of params) {
        const control = controlFor(view, param, values)

        if (param.kind === 'range') {
          expect(control.tagName, param.key).toBe('INPUT')
          expect(control.getAttribute('type'), param.key).toBe('range')
        } else if (param.kind === 'color') {
          // Two routes into one value: the swatch on the bar, the hex field
          // behind the button.
          expect(control.getAttribute('type')).toBe('color')
          expect(view.getByLabelText(t('tools.param.colorHex'))).not.toBeNull()
        } else if (iconChoice(param)) {
          expect(control.getAttribute('role'), param.key).toBe('radiogroup')
          const cells = [...control.querySelectorAll('[role="radio"]')]
          expect(cells).toHaveLength(param.options.length)
          // A glyph and no words: the label is the cell's accessible name.
          for (const [index, cell] of cells.entries()) {
            expect(cell.querySelector('svg'), param.key).not.toBeNull()
            expect(cell.textContent?.trim()).toBe('')
            expect(cell.getAttribute('aria-label')).toBe(t(param.options[index].labelKey))
          }
        } else {
          expect(control.tagName, param.key).toBe('BUTTON')
          expect(control.getAttribute('aria-haspopup'), param.key).toBe('menu')
          // The short label is what is drawn, where the spec asked for one.
          expect(control.textContent).toContain(t(param.shortKey ?? param.labelKey))
        }
      }
    })
  }
})

describe('a change on any control writes that parameter, once', () => {
  for (const spec of TOOL_SPECS) {
    it(`writes every parameter of ${spec.id}`, async () => {
      const { view, values } = mountTool(spec)
      const params = activeParams(spec, values)
      if (params.some((param) => !onBar(param))) await openAdjustments(view)

      for (const param of params) {
        stores.setToolParam.mockClear()
        const control = controlFor(view, param, values)

        if (param.kind === 'range') {
          const next = Math.min(param.max, param.min + param.step)
          await fireEvent.input(control, { target: { value: String(next) } })
          expect(stores.setToolParam).toHaveBeenCalledTimes(1)
          expect(stores.setToolParam).toHaveBeenCalledWith(spec.id, param.key, next)
        } else if (param.kind === 'color') {
          await fireEvent.input(control, { target: { value: '#123456' } })
          expect(stores.setToolParam).toHaveBeenCalledTimes(1)
          expect(stores.setToolParam).toHaveBeenCalledWith(spec.id, param.key, '#123456')
        } else {
          const next = param.options[1]?.value
          expect(next, `${param.key} has a second option to move to`).toBeTruthy()
          if (iconChoice(param)) {
            const cells = [...control.querySelectorAll('[role="radio"]')]
            await fireEvent.click(cells[1])
          } else {
            await fireEvent.click(control)
            const items = [...view.container.querySelectorAll('[role="menuitemradio"]')]
            expect(items.length).toBe(param.options.length)
            await fireEvent.click(items[1])
          }
          expect(stores.setToolParam).toHaveBeenCalledTimes(1)
          expect(stores.setToolParam).toHaveBeenCalledWith(spec.id, param.key, next)
        }
      }
    })
  }
})

describe('the hex field says when what is in it is not a colour', () => {
  const shapes = toolSpec('shapes')

  /** @returns {Promise<HTMLInputElement>} */
  async function hexField(view) {
    await openAdjustments(view)
    return /** @type {HTMLInputElement} */ (view.getByLabelText(t('tools.param.colorHex')))
  }

  it('starts clean, with the committed colour in it and no note', async () => {
    const { view } = mountTool(shapes)
    const field = await hexField(view)
    expect(field.value).toBe('#000000')
    expect(field.getAttribute('aria-invalid')).toBeNull()
    expect(view.container.querySelector('[data-hex-invalid]')).toBeNull()
  })

  it('marks the field and shows the note while the text is not a colour', async () => {
    const { view } = mountTool(shapes)
    const field = await hexField(view)
    await fireEvent.input(field, { target: { value: '#ab' } })

    expect(field.getAttribute('aria-invalid')).toBe('true')
    const note = view.container.querySelector('[data-hex-invalid]')
    expect(note?.textContent?.trim()).toBe(t('tools.param.colorHexInvalid'))
    expect(field.getAttribute('aria-describedby')).toBe(note?.id)
    // Nothing was written: the swatch still holds the colour in force.
    expect(stores.setToolParam).not.toHaveBeenCalled()
  })

  it('clears the note as soon as the text becomes a colour, and commits it', async () => {
    const { view } = mountTool(shapes)
    const field = await hexField(view)
    await fireEvent.input(field, { target: { value: '#ab' } })
    await fireEvent.input(field, { target: { value: '#abc' } })

    expect(field.getAttribute('aria-invalid')).toBeNull()
    expect(view.container.querySelector('[data-hex-invalid]')).toBeNull()
    // Three digits are a colour on *commit*, not while typing - Enter is what
    // expands `#abc`, and until then nothing has been written.
    expect(stores.setToolParam).not.toHaveBeenCalled()

    await fireEvent.keyDown(field, { key: 'Enter' })
    expect(stores.setToolParam).toHaveBeenCalledTimes(1)
    expect(stores.setToolParam).toHaveBeenCalledWith('shapes', 'color', '#aabbcc')
  })

  it('commits 3-digit shorthand expanding to 6 digits on blur', async () => {
    const { view } = mountTool(shapes)
    const field = await hexField(view)
    await fireEvent.input(field, { target: { value: '#fff' } })
    expect(stores.setToolParam).not.toHaveBeenCalled()

    await fireEvent.blur(field)
    expect(stores.setToolParam).toHaveBeenCalledTimes(1)
    expect(stores.setToolParam).toHaveBeenCalledWith('shapes', 'color', '#ffffff')
  })

  it('reverts invalid hex text to the committed colour on blur without writing', async () => {
    const { view } = mountTool(shapes)
    const field = await hexField(view)
    await fireEvent.input(field, { target: { value: '#ab' } })
    expect(field.getAttribute('aria-invalid')).toBe('true')
    expect(view.container.querySelector('[data-hex-invalid]')).not.toBeNull()

    await fireEvent.blur(field)
    expect(stores.setToolParam).not.toHaveBeenCalled()
    expect(field.value).toBe('#000000')
    expect(field.getAttribute('aria-invalid')).toBeNull()
    expect(view.container.querySelector('[data-hex-invalid]')).toBeNull()
  })

  it('does not mark an empty hex field as invalid and reverts on blur', async () => {
    const { view } = mountTool(shapes)
    const field = await hexField(view)
    await fireEvent.input(field, { target: { value: '' } })
    expect(field.getAttribute('aria-invalid')).toBeNull()
    expect(view.container.querySelector('[data-hex-invalid]')).toBeNull()

    await fireEvent.blur(field)
    expect(stores.setToolParam).not.toHaveBeenCalled()
    expect(field.value).toBe('#000000')
  })
})

describe('the eyedropper is drawn only where the platform has one', () => {
  const shapes = toolSpec('shapes')

  it('is absent without `EyeDropper`, which is every platform but Chromium', async () => {
    const { view } = mountTool(shapes)
    await openAdjustments(view)
    expect(view.queryByLabelText(t('tools.action.eyedropper'))).toBeNull()
  })

  it('writes the colour it picked when the platform has one', async () => {
    const opened = vi.fn(async () => ({ sRGBHex: '#0a0b0c' }))
    // @ts-expect-error - the global exists on Chromium alone
    globalThis.EyeDropper = class {
      open() {
        return opened()
      }
    }
    try {
      const { view } = mountTool(shapes)
      await openAdjustments(view)
      await fireEvent.click(view.getByLabelText(t('tools.action.eyedropper')))
      expect(opened).toHaveBeenCalledTimes(1)
      expect(stores.setToolParam).toHaveBeenCalledWith('shapes', 'color', '#0a0b0c')
    } finally {
      // @ts-expect-error - putting the platform back as it was
      delete globalThis.EyeDropper
    }
  })
})

/**
 * A blocked option is shown disabled **and says why**, because a `title` on a
 * disabled control reaches nobody - browsers fire no hover events on one. The
 * reason has two homes, and which one it takes is decided by the control: a
 * dropdown holds it inside itself, under its items; an icon group has no
 * inside, so it sits beside the group on the bar.
 */
describe('a gated option says what it is costing the user', () => {
  it('disables the cloud engine and prints the reason beside the group', () => {
    stores.session.cloudAllowed = false
    try {
      const { view } = mountTool(toolSpec('contentAwareFill'))
      const group = view.getByLabelText(t('tools.param.engine'))
      const cells = [...group.querySelectorAll('[role="radio"]')]
      expect(cells[1].getAttribute('aria-label')).toBe(t('tools.option.engineCloud'))
      expect(/** @type {HTMLButtonElement} */ (cells[1]).disabled).toBe(true)
      expect(view.getByText(t('editor.state.cloudBlocked'))).not.toBeNull()
    } finally {
      stores.session.cloudAllowed = true
    }
  })

  it('prints a withheld rung’s reason inside the dropdown that withheld it', async () => {
    stores.capabilities.sidecar = false
    stores.capabilities.sidecarReasonKey = 'decline.reason.sidecarMachine'
    stores.capabilities.engines = { flux: false }
    try {
      const spec = toolSpec('aiMaskBrush')
      const { view, values } = mountTool(spec)
      const trigger = controlFor(view, spec.params[0], values)
      await fireEvent.click(trigger)

      // Beside the list rather than in it - a `role="menu"` may hold menu
      // items and separators and nothing else - and the list is described by
      // it, which is what carries the sentence to a screen reader now that it
      // is not a child that could be read on the way past.
      const menu = /** @type {HTMLElement} */ (view.container.querySelector('[role="menu"]'))
      expect(menu.querySelector('p')).toBeNull()
      const note = document.getElementById(String(menu.getAttribute('aria-describedby')))
      expect(note?.textContent?.trim()).toBe(t('decline.reason.sidecarMachine'))
      expect(note?.parentElement?.contains(menu)).toBe(true)
      // The trigger carries it too, for a pointer that never opens the menu.
      expect(trigger.getAttribute('title')).toBe(t('decline.reason.sidecarMachine'))
    } finally {
      stores.capabilities.sidecar = true
      stores.capabilities.sidecarReasonKey = null
      stores.capabilities.engines = {}
    }
  })
})

describe('the run action', () => {
  it('is Auto clean’s alone', () => {
    const { view } = mountTool(toolSpec('autoClean'))
    expect(view.getByText(t('editor.action.runOnPage'))).not.toBeNull()

    cleanup()
    const { view: brush } = mountTool(toolSpec('brush'))
    expect(brush.queryByText(t('editor.action.runOnPage'))).toBeNull()
  })

  it('is disabled with a reason when the models are not downloaded', () => {
    stores.capabilities.autoClean = false
    try {
      const { view } = mountTool(toolSpec('autoClean'))
      const action = /** @type {HTMLButtonElement} */ (
        view.getByText(t('editor.action.runOnPage')).closest('button')
      )
      expect(action.disabled).toBe(true)
      const note = view.getByText(t('editor.state.modelsMissing'))
      // And the note is the button's description - a control refused with the
      // reason only on screen is a control refused for no stated reason.
      expect(action.getAttribute('aria-describedby')).toBe(note.id)
      expect(note.id).not.toBe('')
    } finally {
      stores.capabilities.autoClean = true
    }
  })

  it('starts the run on the scope the parameter holds', async () => {
    const spec = toolSpec('autoClean')
    const { view } = mountTool(spec)

    await fireEvent.click(view.getByText(t('editor.action.runOnPage')))
    expect(stores.startRun).toHaveBeenCalledTimes(1)
    expect(stores.startRun).toHaveBeenCalledWith('page')

    // The label follows the scope, and so does what the press starts. The
    // stub stores are plain objects, so the second scope is a second mount
    // rather than a write the bar would react to.
    cleanup()
    stores.startRun.mockClear()
    const { view: project } = mountTool(spec, { scope: 'project' })
    await fireEvent.click(project.getByText(t('editor.action.runOnProject')))
    expect(stores.startRun).toHaveBeenCalledTimes(1)
    expect(stores.startRun).toHaveBeenCalledWith('project')
    expect(stores.cancelRun).not.toHaveBeenCalled()
  })

  it('cancels rather than starts while a run is going, and says which page', async () => {
    stores.editor.run.active = true
    try {
      const { view } = mountTool(toolSpec('autoClean'))
      // `runningPage()` is null in these stubs, so the line names the page on
      // screen - which is the honest answer for the moment before the first
      // `page-started`.
      const live = /** @type {HTMLElement} */ (
        view.container.querySelector('[aria-live="polite"]')
      )
      expect(live.textContent).toBe(t('editor.status.cleaning', { page: 1 }))

      await fireEvent.click(view.getByText(t('editor.action.cancelRun')))
      expect(stores.cancelRun).toHaveBeenCalledTimes(1)
      expect(stores.startRun).not.toHaveBeenCalled()
    } finally {
      stores.editor.run.active = false
    }
  })

  // The line beside the button is a live region, so it is on the page before
  // there is anything in it - an `aria-live` element added at the moment its
  // text arrives announces nothing.
  it('keeps an empty live region while nothing is running', () => {
    const { view } = mountTool(toolSpec('autoClean'))
    const live = view.container.querySelector('[aria-live="polite"]')
    expect(live).not.toBeNull()
    expect(live?.textContent).toBe('')
  })
})

describe('the bar is as long as its contents', () => {
  it('sets no width of its own, on any tool', () => {
    for (const spec of TOOL_SPECS) {
      const { view } = mountTool(spec)
      const bar = /** @type {HTMLElement} */ (view.container.querySelector('section'))
      expect(bar.style.width, spec.id).toBe('')
      // Position and stacking it does own; the width is the element's.
      expect(bar.style.left).toBe('16px')
      expect(bar.style.top).toBe('62px')
      cleanup()
    }
  })

  /**
   * jsdom ships no `ResizeObserver`, so the bar's measuring effect does
   * nothing there unless one is put in front of it. This is the smallest one
   * that is still the contract: it hands the callback back and the test fires
   * it with a box.
   */
  it('writes the border box it measured back through `setWindowBox`', async () => {
    /** @type {((entries: unknown[]) => void)|null} */
    let notify = null
    const global = /** @type {any} */ (globalThis)
    const previous = global.ResizeObserver
    global.ResizeObserver = class {
      constructor(/** @type {(entries: unknown[]) => void} */ callback) {
        notify = callback
      }
      observe() {}
      disconnect() {}
    }
    try {
      mountTool(toolSpec('autoClean'))
      expect(notify).not.toBeNull()

      // The **border** box: `contentRect` stops inside the bar's padding, and
      // the number `clampPosition` places the bar by is the whole box.
      notify?.([{ borderBoxSize: [{ inlineSize: 412.4 }], contentRect: { width: 400 } }])
      expect(stores.setWindowBox).toHaveBeenCalledWith('tool', { w: 412 })

      // A bar being torn down measures 0, and that is not a width to place
      // anything by.
      stores.setWindowBox.mockClear()
      notify?.([{ borderBoxSize: [{ inlineSize: 0 }], contentRect: { width: 0 } }])
      expect(stores.setWindowBox).not.toHaveBeenCalled()
    } finally {
      global.ResizeObserver = previous
    }
  })
})

/**
 * `Tab` ends a menu, because a menu is one control. It must **not** end the
 * Adjustments popover, which is a panel of several: the first Tab off the
 * first slider used to take the whole panel with it. What ends the popover is
 * focus leaving the anchor.
 */
describe('the Adjustments popover keeps Tab to itself', () => {
  /**
   * The open panel and the controls in it. Shapes is the tool with the fullest
   * one - two sliders and the hex field - which is what makes a Tab *between*
   * controls a thing that can happen at all.
   *
   * jsdom gives every element a null `offsetParent`, so `ui/focus.js#focusable`
   * finds nothing and the panel's opening focus never moves; the first control
   * is focused here instead, which is the state a browser would already be in.
   *
   * @param {import('@testing-library/svelte').RenderResult} view
   */
  async function openPanel(view) {
    await openAdjustments(view)
    const panel = /** @type {HTMLElement} */ (view.container.querySelector('[role="dialog"]'))
    const controls = /** @type {HTMLElement[]} */ ([
      ...panel.querySelectorAll('input, button, select, textarea'),
    ])
    expect(controls.length, 'the panel holds more than one control').toBeGreaterThan(1)
    controls[0].focus()
    return controls
  }

  it('stays open while Tab moves between its own controls', async () => {
    const { view } = mountTool(toolSpec('shapes'))
    const controls = await openPanel(view)

    await fireEvent.keyDown(controls[0], { key: 'Tab' })
    // jsdom moves no focus of its own on Tab, so the move the browser would
    // make is made here - and it is that move, not the key, that the popover
    // reads.
    controls[1].focus()
    await fireEvent.focusOut(controls[0], { relatedTarget: controls[1] })

    expect(view.container.querySelector('[role="dialog"]')).not.toBeNull()
    expect(document.activeElement).toBe(controls[1])
  })

  it('closes when focus lands outside the anchor', async () => {
    const { view } = mountTool(toolSpec('shapes'))
    const controls = await openPanel(view)

    const outside = document.createElement('button')
    document.body.append(outside)
    try {
      outside.focus()
      await fireEvent.focusOut(controls.at(-1) ?? controls[0], { relatedTarget: outside })
      expect(view.container.querySelector('[role="dialog"]')).toBeNull()
    } finally {
      outside.remove()
    }
  })

  // WebKit - the engine under the shipped Tauri application - does not focus
  // a button or a range input on click, so a press on the panel's own slider
  // thumb, label or padding blurs the focused control with a null
  // `relatedTarget`. That is not focus leaving for somewhere else, and the
  // panel must stay; a press that really is outside is the outside
  // pointerdown listener's to answer, exactly as it is for a menu.
  it('stays open when focus is dropped rather than moved elsewhere', async () => {
    const { view } = mountTool(toolSpec('shapes'))
    const controls = await openPanel(view)

    await fireEvent.focusOut(controls[0], { relatedTarget: null })

    expect(view.container.querySelector('[role="dialog"]')).not.toBeNull()
  })
})

describe('tool switching resets transient state and closes overlays', () => {
  it('closes an open dropdown menu when the tool changes', async () => {
    const { view } = mountTool(toolSpec('autoClean'))
    const dropdown = view.getByLabelText(
      t('tools.label.choice', {
        labelKey: 'tools.param.bubbleText',
        value: t('masks.engineChoice.fill'),
      }),
    )
    await fireEvent.click(dropdown)
    expect(view.container.querySelector('[role="menu"]')).not.toBeNull()

    stores.editor.tool = 'brush'
    stores.editor.toolParams = {
      ...stores.editor.toolParams,
      brush: startingValues(toolSpec('brush')),
    }
    await tick()

    expect(view.container.querySelector('[role="menu"]')).toBeNull()
  })

  it('closes an open Adjustments popover when the tool changes', async () => {
    const { view } = mountTool(toolSpec('shapes'))
    await openAdjustments(view)
    expect(view.container.querySelector('[role="dialog"]')).not.toBeNull()

    stores.editor.tool = 'autoClean'
    stores.editor.toolParams = {
      ...stores.editor.toolParams,
      autoClean: startingValues(toolSpec('autoClean')),
    }
    await tick()

    expect(view.container.querySelector('[role="dialog"]')).toBeNull()
  })

  it('does not leak half-typed hex text across tool switches', async () => {
    const { view } = mountTool(toolSpec('shapes'))
    await openAdjustments(view)
    const field = /** @type {HTMLInputElement} */ (view.getByLabelText(t('tools.param.colorHex')))
    await fireEvent.input(field, { target: { value: '#123456' } })
    expect(field.value).toBe('#123456')

    stores.editor.tool = 'brush'
    stores.editor.toolParams = {
      ...stores.editor.toolParams,
      brush: { ...startingValues(toolSpec('brush')), mode: 'paint', color: '#ff0000' },
    }
    await tick()

    await openAdjustments(view)
    const brushField = /** @type {HTMLInputElement} */ (
      view.getByLabelText(t('tools.param.colorHex'))
    )
    expect(brushField.value).toBe('#ff0000')
  })
})

describe('the Brush tool in paint mode draws and writes color, opacity, and flow', () => {
  it('shows swatch on the bar and opacity/flow in Adjustments', async () => {
    const brush = toolSpec('brush')
    const { view } = mountTool(brush, { mode: 'paint', color: '#336699', opacity: 80, flow: 90 })

    const swatch = view.getByLabelText(t('tools.param.color'))
    expect(swatch).not.toBeNull()
    expect(swatch.getAttribute('type')).toBe('color')
    expect(/** @type {HTMLInputElement} */ (swatch).value).toBe('#336699')

    await openAdjustments(view)

    const opacity = view.getByLabelText(t('tools.param.opacity'))
    expect(opacity).not.toBeNull()
    await fireEvent.input(opacity, { target: { value: '50' } })
    expect(stores.setToolParam).toHaveBeenCalledWith('brush', 'opacity', 50)

    const flow = view.getByLabelText(t('tools.param.flow'))
    expect(flow).not.toBeNull()
    await fireEvent.input(flow, { target: { value: '60' } })
    expect(stores.setToolParam).toHaveBeenCalledWith('brush', 'flow', 60)
  })
})

describe('toolbar gestures and window controls', () => {
  it('moves the bar with arrow keys on the grip and prevents default', async () => {
    const { view } = mountTool(toolSpec('autoClean'))
    const grip = view.getByLabelText(
      t('editor.window.move', { windowName: t('tools.name.autoClean') }),
    )

    const event = new KeyboardEvent('keydown', {
      key: 'ArrowRight',
      bubbles: true,
      cancelable: true,
    })
    const preventDefaultSpy = vi.spyOn(event, 'preventDefault')
    grip.dispatchEvent(event)

    expect(preventDefaultSpy).toHaveBeenCalled()
    expect(stores.setWindowBox).toHaveBeenCalledWith(
      'tool',
      expect.objectContaining({ x: expect.any(Number) }),
    )
    expect(stores.commitWindows).toHaveBeenCalled()
  })

  it('closes the tool bar when clicking the close button', async () => {
    const { view } = mountTool(toolSpec('autoClean'))
    const closeBtn = view.getByLabelText(
      t('editor.window.close', { windowName: t('tools.name.autoClean') }),
    )
    await fireEvent.click(closeBtn)
    expect(stores.setWindowOpen).toHaveBeenCalledWith('tool', false)
  })

  it('closes the Adjustments popover on Escape and stops propagation', async () => {
    const { view } = mountTool(toolSpec('shapes'))
    await openAdjustments(view)
    expect(view.container.querySelector('[role="dialog"]')).not.toBeNull()

    const panel = /** @type {HTMLElement} */ (view.container.querySelector('[role="dialog"]'))
    const slider = panel.querySelector('input')
    expect(slider).not.toBeNull()

    const escapeEvent = new KeyboardEvent('keydown', {
      key: 'Escape',
      bubbles: true,
      cancelable: true,
    })
    const stopPropagationSpy = vi.spyOn(escapeEvent, 'stopPropagation')
    slider?.dispatchEvent(escapeEvent)
    await tick()

    expect(stopPropagationSpy).toHaveBeenCalled()
    expect(view.container.querySelector('[role="dialog"]')).toBeNull()
  })
})

describe('sync effect handles edge cases safely', () => {
  it('does not write to store when choice options list is empty', async () => {
    stores.capabilities.engines = {
      fill: false,
      migan: false,
      lama: false,
      ldm: false,
      flux: false,
    }
    try {
      mountTool(toolSpec('aiMaskBrush'))
      await tick()
      expect(stores.setToolParam).not.toHaveBeenCalled()
    } finally {
      stores.capabilities.engines = {}
    }
  })
})
