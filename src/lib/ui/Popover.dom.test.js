/**
 * DOM tests for ui components: Popover, Menu, Segmented, Slider, and overlay.
 *
 * Runs in the vitest jsdom environment via the `.dom.test.js` suffix.
 */

import { describe, expect, it, afterEach } from 'vitest'
import { render, cleanup, fireEvent } from '@testing-library/svelte'
import { createRawSnippet, mount, unmount } from 'svelte'
import Popover from './Popover.svelte'
import Menu from './Menu.svelte'
import Segmented from './Segmented.svelte'
import Slider from './Slider.svelte'
import { captureFocus } from './focus.js'

afterEach(() => {
  cleanup()
})

describe('Popover', () => {
  it('mounts with trigger, opens panel, sets aria attributes, width and cursor', async () => {
    let getTriggerProps
    const trigger = createRawSnippet((getProps) => {
      getTriggerProps = getProps
      return {
        render: () => '<button type="button" class="test-trigger">Open</button>',
        setup: (node) => {
          node.onclick = () => {
            getProps().toggle()
          }
        },
      }
    })

    const children = createRawSnippet(() => ({
      render: () => '<input type="range" class="test-slider" />',
    }))

    const view = render(Popover, {
      props: {
        label: 'Test Popover',
        width: 320,
        trigger,
        children,
      },
    })

    const triggerBtn = view.container.querySelector('.test-trigger')
    expect(view.container.querySelector('[role="dialog"]')).toBeNull()

    await fireEvent.click(triggerBtn)

    const dialog = view.container.querySelector('[role="dialog"]')
    expect(dialog).not.toBeNull()
    expect(dialog.getAttribute('aria-label')).toBe('Test Popover')
    expect(dialog.getAttribute('aria-modal')).toBeNull()
    expect(dialog.style.width).toBe('320px')

    // Verify triggerProps
    const props = getTriggerProps()
    expect(props.open).toBe(true)
    expect(props.triggerProps['aria-controls']).toBe(dialog.id)
    expect(props.triggerProps['aria-haspopup']).toBe('dialog')
    expect(props.triggerProps['aria-expanded']).toBe(true)
  })

  it('ends closed when trigger is clicked while open in WebKit (focusout to body then click toggle)', async () => {
    const trigger = createRawSnippet((getProps) => ({
      render: () => '<button type="button" class="test-trigger">Open</button>',
      setup: (node) => {
        node.onclick = () => {
          getProps().toggle()
        }
      },
    }))

    const children = createRawSnippet(() => ({
      render: () => '<input type="range" class="test-slider" />',
    }))

    const view = render(Popover, {
      props: {
        label: 'Adjustments',
        trigger,
        children,
      },
    })

    const triggerBtn = view.container.querySelector('.test-trigger')
    // Open the popover
    await fireEvent.click(triggerBtn)
    expect(view.container.querySelector('[role="dialog"]')).not.toBeNull()

    const slider = view.container.querySelector('.test-slider')

    // Simulate WebKit trace:
    // 1. pointerdown on trigger while open
    await fireEvent.pointerDown(triggerBtn)
    // 2. outside listener runs (target is in root, does not close)
    // 3. WebKit blurs slider with relatedTarget = document.body (or null)
    await fireEvent.focusOut(slider, { relatedTarget: document.body })
    // 4. click on trigger fires toggle()
    await fireEvent.click(triggerBtn)

    // MUST END CLOSED!
    expect(view.container.querySelector('[role="dialog"]')).toBeNull()
  })

  it('closes only the inner menu on Escape when Menu is nested inside Popover', async () => {
    let menuInstance
    const menuTrigger = createRawSnippet((getProps) => ({
      render: () => '<button type="button" class="menu-trigger">Menu</button>',
      setup: (node) => {
        node.onclick = () => getProps().toggle()
      },
    }))

    const popoverChildren = createRawSnippet(() => ({
      render: () => '<div class="menu-slot"></div>',
      setup: (slotNode) => {
        menuInstance = mount(Menu, {
          target: slotNode,
          props: {
            label: 'Inner Menu',
            items: [
              { id: 'opt1', label: 'Option 1' },
              { id: 'opt2', label: 'Option 2' },
            ],
            onselect: () => {},
            trigger: menuTrigger,
          },
        })
        return () => {
          if (menuInstance) {
            unmount(menuInstance)
            menuInstance = null
          }
        }
      },
    }))

    const popoverTrigger = createRawSnippet((getProps) => ({
      render: () => '<button type="button" class="popover-trigger">Popover</button>',
      setup: (node) => {
        node.onclick = () => {
          getProps().toggle()
        }
      },
    }))

    const view = render(Popover, {
      props: {
        label: 'Outer Popover',
        trigger: popoverTrigger,
        children: popoverChildren,
      },
    })

    // Open Popover
    await fireEvent.click(view.container.querySelector('.popover-trigger'))
    expect(view.container.querySelector('[role="dialog"]')).not.toBeNull()

    // Open Menu
    const menuBtn = view.container.querySelector('.menu-trigger')
    await fireEvent.click(menuBtn)
    expect(view.container.querySelector('[role="menu"]')).not.toBeNull()

    // Press Escape inside Menu
    const menuItem = view.container.querySelector('[role="menuitem"]')
    await fireEvent.keyDown(menuItem, { key: 'Escape' })

    // Menu must be closed, but Popover must remain open!
    expect(view.container.querySelector('[role="menu"]')).toBeNull()
    expect(view.container.querySelector('[role="dialog"]')).not.toBeNull()

    // Press Escape again on Popover
    await fireEvent.keyDown(view.container.querySelector('.anchor'), { key: 'Escape' })
    expect(view.container.querySelector('[role="dialog"]')).toBeNull()
  })

  it('restores focus to replacement element in container if original element was removed', () => {
    const container = document.createElement('div')
    document.body.appendChild(container)
    const oldBtn = document.createElement('button')
    oldBtn.className = 'old-btn'
    container.appendChild(oldBtn)
    oldBtn.focus()
    expect(document.activeElement).toBe(oldBtn)

    const restore = captureFocus(() => container)

    // Remove oldBtn and replace with newBtn
    oldBtn.remove()
    const newBtn = document.createElement('button')
    newBtn.className = 'new-btn'
    Object.defineProperty(newBtn, 'offsetParent', { value: container, configurable: true })
    container.appendChild(newBtn)

    // Restore focus
    restore()
    expect(document.activeElement).toBe(newBtn)
    container.remove()
  })
})

describe('Menu', () => {
  const makeTrigger = (className) =>
    createRawSnippet((getProps) => ({
      render: () => `<button type="button" class="${className}">Trigger</button>`,
      setup: (node) => {
        node.onclick = () => getProps().toggle()
      },
    }))

  it('renders note wrapper and noteId only when note is provided', async () => {
    // Menu without note
    const viewWithoutNote = render(Menu, {
      props: {
        label: 'No Note Menu',
        items: [{ id: 'a', label: 'Alpha' }],
        onselect: () => {},
        trigger: makeTrigger('trig-no-note'),
      },
    })
    await fireEvent.click(viewWithoutNote.container.querySelector('.trig-no-note'))
    const menuWithoutNote = viewWithoutNote.container.querySelector('.menu')
    expect(menuWithoutNote.getAttribute('role')).toBe('menu')
    expect(viewWithoutNote.container.querySelector('.list')).toBeNull()
    expect(viewWithoutNote.container.querySelector('.note')).toBeNull()

    // Menu with note
    const viewWithNote = render(Menu, {
      props: {
        label: 'With Note Menu',
        note: 'Requires engine download',
        items: [{ id: 'b', label: 'Beta', disabled: true }],
        onselect: () => {},
        trigger: makeTrigger('trig-with-note'),
      },
    })
    await fireEvent.click(viewWithNote.container.querySelector('.trig-with-note'))
    const outerWithNote = viewWithNote.container.querySelector('.menu')
    expect(outerWithNote.getAttribute('role')).toBeNull() // outer is wrapper, not role="menu"
    const innerList = viewWithNote.container.querySelector('.list')
    expect(innerList).not.toBeNull()
    expect(innerList.getAttribute('role')).toBe('menu')
    const noteEl = viewWithNote.container.querySelector('.note')
    expect(noteEl).not.toBeNull()
    expect(noteEl.textContent).toBe('Requires engine download')
    expect(innerList.getAttribute('aria-describedby')).toBe(noteEl.id)
    const disabledItem = viewWithNote.container.querySelector('[role="menuitem"]:disabled')
    expect(disabledItem.getAttribute('aria-describedby')).toBe(noteEl.id)
  })
})

describe('Segmented', () => {
  it('renders icon glyph cells with accessible names and describedBy', async () => {
    let selected = 'rect'
    const view = render(Segmented, {
      props: {
        options: [
          { value: 'rect', label: 'Rectangle', icon: 'shape-rect' },
          { value: 'ellipse', label: 'Ellipse', icon: 'shape-ellipse', disabled: true },
        ],
        value: selected,
        describedBy: 'gate-note-1',
        onchange: (v) => {
          selected = v
        },
      },
    })

    const group = view.container.querySelector('[role="radiogroup"]')
    expect(group.getAttribute('aria-describedby')).toBe('gate-note-1')

    const radios = view.container.querySelectorAll('[role="radio"]')
    expect(radios.length).toBe(2)

    // Cell 0: icon cell
    expect(radios[0].classList.contains('glyph')).toBe(true)
    expect(radios[0].getAttribute('aria-label')).toBe('Rectangle')
    expect(radios[0].getAttribute('title')).toBe('Rectangle')
    expect(radios[0].getAttribute('tabindex')).toBe('0')

    // Cell 1: disabled
    expect(radios[1].hasAttribute('disabled')).toBe(true)
    expect(radios[1].getAttribute('aria-label')).toBe('Ellipse')
    expect(radios[1].getAttribute('title')).toBe('Ellipse')
    expect(radios[1].getAttribute('tabindex')).toBe('-1')
  })
})

describe('Slider', () => {
  it('compact variant renders track, label association, and formatted readout', async () => {
    let val = 24
    const view = render(Slider, {
      props: {
        compact: true,
        label: 'Size',
        value: val,
        min: 1,
        max: 100,
        unit: 'px',
        onchange: (v) => {
          val = v
        },
      },
    })

    const input = view.container.querySelector('input[type="range"]')
    const label = view.container.querySelector('label')
    const output = view.container.querySelector('output')

    expect(input.id).toBeTruthy()
    expect(label.getAttribute('for')).toBe(input.id)
    expect(output.getAttribute('for')).toBe(input.id)
    expect(input.getAttribute('aria-valuetext')).toBe('24px')
    expect(output.textContent).toBe('24px')

    const row = view.container.querySelector('.row')
    expect(row.classList.contains('compact')).toBe(true)
  })
})
