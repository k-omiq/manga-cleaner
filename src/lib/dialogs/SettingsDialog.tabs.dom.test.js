/**
 * The Settings dialog's tab strip, and the one binding in it that is not a key.
 *
 * The strip replaced a single scrolling body, and the argument that body was
 * built on is written out at the top of `SettingsDialog.svelte`: five
 * preference rows must not move further from the hand, the dialog must not
 * change height as panels swap, the strip must be a real WAI-ARIA tab list
 * rather than five buttons that look like one, and the heading outline must
 * survive. Three of those four are assertable here and are asserted here; the
 * height is a fixed CSS box (`.panel`) that jsdom does not lay out, so it is
 * checked in a browser instead.
 *
 * The second half of the file is the pointer modifier - `session
 * .cloneSourceModifier`, the setting that exists because *Alt* is not a key on
 * an Apple keyboard. What is asserted is the round trip the user actually
 * makes: press a chip in Settings › Shortcuts, and the tool window's own hint
 * says the new modifier. The arithmetic underneath it - which modifier a
 * pointer event satisfies - is pure and lives in `shortcuts.test.js`.
 *
 * The seam is stubbed the way the two files beside this one stub it: the
 * dialog opens six calls on mount and none of them is what this file is about,
 * so each answers the emptiest true thing.
 */

import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { cleanup, fireEvent, render } from '@testing-library/svelte'

import { setBackend } from '../api/backend.js'
import { t } from '../i18n/index.js'
import { DEFAULT_POINTER_MODIFIER, isApplePlatform, modifierCap } from '../shortcuts.js'
import { session, setCloneSourceModifier } from '../state/session.svelte.js'
import SettingsDialog from './SettingsDialog.svelte'

/** The spec `pushModal({kind: 'settings'})` would have handed the dialog. */
const SPEC = {
  id: 'modal-1',
  kind: 'settings',
  titleKey: 'modal.title.settings',
  props: {},
  actions: [{ id: 'close', labelKey: 'shell.action.close' }],
  blocking: false,
  dismissable: true,
  onresolve: null,
}

/** The tabs, in the order the strip offers them. */
const TABS = ['general', 'models', 'acceleration', 'shortcuts', 'about']

/** The two panels whose tail holds nothing focusable, so the scroller is the stop. */
const FOCUSABLE_SCROLLERS = new Set(['acceleration', 'about'])

/** @param {string} id */
const tabName = (id) => t(`settings.section.${id}`)

beforeEach(() => {
  setBackend(
    /** @type {any} */ ({
      listModels: vi.fn(async () => null),
      listAccelerators: vi.fn(async () => ({ providers: [], models: [] })),
      listSidecarModels: vi.fn(async () => []),
      sidecarAvailable: vi.fn(async () => ({ available: false, reasonKey: null })),
      about: vi.fn(async () => ({ appVersion: '0.0.0-test', facts: [] })),
      subscribe: vi.fn(() => () => {}),
      writeSettings: vi.fn(async () => ({})),
    }),
  )
})

afterEach(() => {
  cleanup()
  setBackend(null)
  // The session is a module singleton, so a modifier this file chose would
  // otherwise be the modifier the next file mounts with.
  setCloneSourceModifier(DEFAULT_POINTER_MODIFIER)
  vi.clearAllMocks()
})

function open() {
  const rendered = render(SettingsDialog, { props: { spec: SPEC } })
  /** @param {string} id */
  const tab = (id) => rendered.getByRole('tab', { name: tabName(id) })
  /** @param {string} id */
  const panel = (id) =>
    /** @type {HTMLElement} */ (
      rendered.container.querySelector(`#${tab(id).getAttribute('aria-controls')}`)
    )
  return { rendered, tab, panel }
}

describe('the tab strip', () => {
  it('opens on General, so the five preference rows did not move', () => {
    const { rendered, tab, panel } = open()

    expect(tab('general').getAttribute('aria-selected')).toBe('true')
    expect(panel('general').hasAttribute('hidden')).toBe(false)
    // The rows themselves, on screen with no press at all - which is the whole
    // of the answer to "a tab strip puts them one click further away".
    expect(panel('general').textContent).toContain(t('settings.theme.label'))
    expect(panel('general').textContent).toContain(t('settings.language.label'))

    for (const id of TABS.slice(1)) {
      expect(tab(id).getAttribute('aria-selected')).toBe('false')
      expect(panel(id).hasAttribute('hidden')).toBe(true)
    }
    expect(rendered.getByRole('tablist').getAttribute('aria-label')).toBe(
      t('settings.tabs.label'),
    )
  })

  it('wires every tab to a panel that exists and names it back', () => {
    const { tab, panel } = open()
    for (const id of TABS) {
      const controls = tab(id).getAttribute('aria-controls')
      expect(controls).toBeTruthy()
      // Mounted, not conditional: `aria-controls` pointing at nothing is a
      // promise the markup does not keep.
      expect(panel(id)).not.toBeNull()
      expect(panel(id).getAttribute('role')).toBe('tabpanel')
      expect(panel(id).getAttribute('aria-labelledby')).toBe(tab(id).id)
    }
  })

  it('holds exactly one tab stop, and it is the selected tab', async () => {
    const { tab } = open()
    const stops = () => TABS.filter((id) => tab(id).getAttribute('tabindex') === '0')

    expect(stops()).toEqual(['general'])
    await fireEvent.click(tab('shortcuts'))
    expect(stops()).toEqual(['shortcuts'])
  })

  it('moves and selects on the arrows, wrapping at both ends', async () => {
    const { tab } = open()
    const selected = () => TABS.find((id) => tab(id).getAttribute('aria-selected') === 'true')

    await fireEvent.keyDown(tab('general'), { key: 'ArrowRight' })
    expect(selected()).toBe('models')
    expect(document.activeElement).toBe(tab('models'))

    await fireEvent.keyDown(tab('models'), { key: 'ArrowLeft' })
    expect(selected()).toBe('general')

    // Left from the first is the last, and right from the last is the first.
    await fireEvent.keyDown(tab('general'), { key: 'ArrowLeft' })
    expect(selected()).toBe('about')
    await fireEvent.keyDown(tab('about'), { key: 'ArrowRight' })
    expect(selected()).toBe('general')
  })

  it('goes to the ends on Home and End', async () => {
    const { tab, panel } = open()

    await fireEvent.keyDown(tab('general'), { key: 'End' })
    expect(tab('about').getAttribute('aria-selected')).toBe('true')
    expect(panel('about').hasAttribute('hidden')).toBe(false)
    expect(document.activeElement).toBe(tab('about'))

    await fireEvent.keyDown(tab('about'), { key: 'Home' })
    expect(tab('general').getAttribute('aria-selected')).toBe('true')
    expect(panel('about').hasAttribute('hidden')).toBe(true)
  })

  it('leaves a key it does not answer to whatever is under the dialog', async () => {
    const { tab } = open()
    const event = new KeyboardEvent('keydown', { key: 'k', bubbles: true, cancelable: true })
    tab('general').dispatchEvent(event)
    expect(event.defaultPrevented).toBe(false)
    expect(tab('general').getAttribute('aria-selected')).toBe('true')
  })

  it('keeps a real heading outline: h2 title, h3 panel, h4 shortcut groups', () => {
    const { rendered, panel } = open()

    expect(rendered.container.querySelectorAll('h2')).toHaveLength(1)
    for (const id of TABS) {
      const heading = panel(id).querySelector('h3')
      expect(heading?.textContent?.trim()).toBe(tabName(id))
    }
    // The sheet's own group headings fall one level below the panel's.
    expect(panel('shortcuts').querySelectorAll('h4').length).toBeGreaterThan(0)
    expect(panel('shortcuts').querySelectorAll('h3')).toHaveLength(1)
  })

  it('makes a panel focusable when, and only when, its tail is not', () => {
    const { panel } = open()
    for (const id of TABS) {
      expect([id, panel(id).getAttribute('tabindex')]).toEqual([
        id,
        FOCUSABLE_SCROLLERS.has(id) ? '0' : null,
      ])
    }
  })
})

describe('the modifier that picks Clone / heal’s source', () => {
  /**
   * The four chips, found by the spelt-out names rather than by their keycaps
   * - `⌥` is not a word, and a role query resolves the name the same way a
   * screen reader would. It is a *role* query, so it will not find a chip in a
   * panel that is still `hidden`: the press has to have happened.
   */
  function chip(rendered, id) {
    return rendered.getByRole('radio', { name: t(`settings.cloneSource.name.${id}`) })
  }

  it('starts on the modifier the gesture has always used', async () => {
    const { rendered, tab, panel } = open()
    await fireEvent.click(tab('shortcuts'))

    expect(session.cloneSourceModifier).toBe('alt')
    expect(chip(rendered, 'alt').getAttribute('aria-checked')).toBe('true')
    // And it is drawn as the key the platform prints, not as the word `Alt`.
    expect(panel('shortcuts').textContent).toContain(
      modifierCap('alt', { apple: isApplePlatform() }),
    )
  })

  it('is changed by a press, and the tool window’s hint says the new one', async () => {
    const { rendered, tab } = open()
    await fireEvent.click(tab('shortcuts'))

    // The string the tool window renders, before and after. It names the
    // modifier through a context param, because `ToolWindow` calls
    // `t(spec.hintKey)` and has no preference to pass.
    const apple = isApplePlatform()
    expect(t('tools.hint.cloneHeal')).toContain(modifierCap('alt', { apple }))

    await fireEvent.click(chip(rendered, 'meta'))

    expect(session.cloneSourceModifier).toBe('meta')
    expect(chip(rendered, 'meta').getAttribute('aria-checked')).toBe('true')
    expect(chip(rendered, 'alt').getAttribute('aria-checked')).toBe('false')
    expect(t('tools.hint.cloneHeal')).toContain(modifierCap('meta', { apple }))
    expect(t('tools.hint.cloneHeal')).not.toContain(modifierCap('alt', { apple }))
  })

  it('offers all four, each with a name a screen reader can say', async () => {
    const { rendered, tab } = open()
    await fireEvent.click(tab('shortcuts'))
    for (const id of ['alt', 'meta', 'control', 'shift']) {
      expect(chip(rendered, id)).toBeTruthy()
    }
  })
})
