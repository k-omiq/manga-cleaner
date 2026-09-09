import { describe, it, expect, beforeEach } from 'vitest'
import {
  app,
  goLibrary,
  openProject,
  openChapter,
  back,
  pushModal,
  replaceModal,
  closeModal,
  closeAllModals,
  activeModal,
  isModalOpen,
} from './app.svelte.js'

beforeEach(() => {
  app.modals.length = 0
  app.notices.length = 0
  app.route = { name: 'library', projectId: null, chapterId: null }
})

describe('routing', () => {
  it('walks the two-level hierarchy', () => {
    openProject('p1')
    expect(app.route).toEqual({ name: 'chapters', projectId: 'p1', chapterId: null })
    openChapter('p1', 'c1')
    expect(app.route).toEqual({ name: 'editor', projectId: 'p1', chapterId: 'c1' })
  })

  it('backs out of the editor to the project, not the library', () => {
    openChapter('p1', 'c1')
    back()
    expect(app.route).toEqual({ name: 'chapters', projectId: 'p1', chapterId: null })
    back()
    expect(app.route.name).toBe('library')
  })

  it('ignores an incomplete destination', () => {
    openProject('')
    expect(app.route.name).toBe('library')
    openChapter('p1', '')
    expect(app.route.name).toBe('library')
  })
})

describe('a route change dismisses the modal stack', () => {
  it('drops every dialog and resolves each with null', () => {
    const resolved = []
    pushModal({ kind: 'settings', onresolve: (r) => resolved.push(['settings', r]) })
    pushModal({ kind: 'about', onresolve: (r) => resolved.push(['about', r]) })
    expect(app.modals.length).toBe(2)

    openProject('p1')

    expect(app.modals.length).toBe(0)
    // Top-down, so a stack unwinds in the order the user would have closed it.
    expect(resolved).toEqual([
      ['about', null],
      ['settings', null],
    ])
  })

  it('leaves the stack alone when the route does not actually change', () => {
    openProject('p1')
    pushModal({ kind: 'settings' })
    openProject('p1')
    expect(app.modals.length).toBe(1)
    goLibrary()
    expect(app.modals.length).toBe(0)
  })

  it('does not recurse when a dropped dialog navigates from its resolver', () => {
    pushModal({ kind: 'newProject', onresolve: () => openProject('p2') })
    openProject('p1')
    // The resolver's own navigation is a real route change and is honoured...
    expect(app.route).toEqual({ name: 'chapters', projectId: 'p2', chapterId: null })
    // ...but it cannot re-enter the drop, because the array was detached first.
    expect(app.modals.length).toBe(0)
  })

  it('keeps a dialog a resolver pushes on the way out', () => {
    pushModal({ kind: 'export', onresolve: () => pushModal({ kind: 'about' }) })
    closeAllModals()
    expect(app.modals.map((m) => m.kind)).toEqual(['about'])
  })
})

describe('the way-out invariant', () => {
  it('gives a dismissable dialog a Close action by default', () => {
    pushModal({ kind: 'settings' })
    expect(activeModal().actions).toEqual([{ id: 'close', labelKey: 'shell.action.close' }])
  })

  it('refuses a non-dismissable dialog with no actions', () => {
    expect(() => pushModal({ kind: 'overwriteRefusal', dismissable: false })).toThrow(TypeError)
    expect(() => pushModal({ kind: 'overwriteRefusal', dismissable: false, actions: [] })).toThrow(
      /no way out/
    )
    expect(() => replaceModal({ kind: 'overwriteRefusal', dismissable: false })).toThrow(TypeError)
    expect(isModalOpen()).toBe(false)
  })

  it('accepts a non-dismissable dialog that names its own way out', () => {
    pushModal({
      kind: 'overwriteRefusal',
      dismissable: false,
      actions: [{ id: 'chooseFolder', labelKey: 'export.action.chooseAnotherFolder' }],
    })
    expect(activeModal().dismissable).toBe(false)
    expect(activeModal().actions).toHaveLength(1)
  })
})

describe('the modal stack', () => {
  it('replaces the top without resolving it - the cloud handoff', () => {
    const resolved = []
    pushModal({ kind: 'settings', onresolve: (r) => resolved.push(['settings', r]) })
    pushModal({ kind: 'cloudTransmission', onresolve: (r) => resolved.push(['transmission', r]) })
    replaceModal({ kind: 'cloudCost', onresolve: (r) => resolved.push(['cost', r]) })

    expect(app.modals.map((m) => m.kind)).toEqual(['settings', 'cloudCost'])
    expect(resolved).toEqual([])

    closeModal('confirm')
    expect(resolved).toEqual([['cost', 'confirm']])
    expect(activeModal().kind).toBe('settings')
  })

  it('pushes onto an empty stack when replacing nothing', () => {
    replaceModal({ kind: 'settings' })
    expect(app.modals).toHaveLength(1)
  })
})
