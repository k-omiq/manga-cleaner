import { describe, expect, it, vi } from 'vitest'

import { flatten, humanise, selectForm, t } from './index.js'
import { keysIn, stripComments, stripIds } from './keyscan.js'

describe('t', () => {
  it('returns a plain entry unchanged', () => {
    expect(t('shell.action.cancel')).toBe('Cancel')
  })

  it('is empty for a missing or non-string key rather than throwing', () => {
    expect(t('')).toBe('')
    expect(t(/** @type {any} */ (null))).toBe('')
    expect(t(/** @type {any} */ (7))).toBe('')
  })

  it('interpolates named params', () => {
    expect(t('home.chapter.number', { number: 12 })).toBe('Ch. 12')
  })

  it('resolves a param whose name ends in Key through the catalogue', () => {
    // `{commandKey}` carries a key, which is resolved through the catalogue.
    expect(t('editor.action.undoCommand', { commandKey: 'masks.command.rerunMask' })).toBe(
      'Undo a mask re-run',
    )
  })

  it('does not resolve a param that merely contains a dot', () => {
    expect(t('notice.project.sourcePathCopied', { path: 'a.b.c' })).toContain('a.b.c')
  })

  it('leaves a placeholder alone when no param is supplied for it', () => {
    // Visible and wrong beats silently empty: a caller that forgot a param sees it.
    expect(t('home.chapter.number')).toBe('Ch. {number}')
    expect(t('home.chapter.number', {})).toBe('Ch. {number}')
  })

  it('renders an explicitly null param as nothing', () => {
    expect(t('home.chapter.number', { number: null })).toBe('Ch. ')
  })
})

describe('the currency format', () => {
  it('is the only way money is written, and keeps three decimals', () => {
    // NB2 @1K is $0.067. Rounding to cents would
    // print $0.07 and overstate the estimate.
    expect(t('masks.value.cloudCost', { cost: 0.067 })).toBe('$0.067')
  })

  it('drops the third decimal when it is not needed', () => {
    expect(t('masks.value.cloudCost', { cost: 1.5 })).toBe('$1.50')
  })

  it('formats zero rather than printing a bare 0', () => {
    expect(t('masks.value.cloudCost', { cost: 0 })).toBe('$0.00')
  })
})

describe('the memory format', () => {
  // A manga-LaMa session is 510 MB resident, which is the
  // largest row the loaded-models tab draws and the one whose rounding is
  // most visible.
  it('rounds to whole megabytes once the figure is large', () => {
    expect(t('models.value.size', { bytes: 510 * 1024 * 1024 })).toBe('510 MB')
  })

  it('keeps one decimal below ten, and turns over to gigabytes at 1024', () => {
    expect(t('models.value.size', { bytes: 3.5 * 1024 * 1024 })).toBe('3.5 MB')
    expect(t('models.value.size', { bytes: 6 * 1024 * 1024 * 1024 })).toBe('6 GB')
  })

  // Base 1024 and not 1000: the file is 94,669,756 bytes, which is 90 MiB.
  it('is base 1024, so a 94.7 MB file reads as 90 MB', () => {
    expect(t('models.value.size', { bytes: 94_669_756 })).toBe('90 MB')
  })

  // The tab is about hundreds of megabytes. A row reading `0.3 MB` invites
  // arithmetic nobody wants to do, and a sidecar that has not reported yet
  // arrives here as zero.
  it('says less than a megabyte rather than a number of kilobytes', () => {
    expect(t('models.value.size', { bytes: 0 })).toBe('< 1 MB')
    expect(t('models.value.size', { bytes: 300 * 1024 })).toBe('< 1 MB')
  })
})

describe('plurals', () => {
  it('reads naturally at zero, which is the common case for a page row', () => {
    const name = t('pages.row.name', {
      label: 'p. 01',
      statusKey: 'pages.status.cleaned',
      count: 0,
      cleaned: 4,
      total: 4,
    })
    expect(name).toBe('p. 01 · cleaned, 4 of 4 regions')
    expect(name).not.toContain('0 ')
  })

  it('uses the singular at one and the plural above it', () => {
    const params = { label: 'p. 02', statusKey: 'pages.status.cleaned', cleaned: 1, total: 3 }
    expect(t('pages.row.name', { ...params, count: 1 })).toContain('1 needs review')
    expect(t('pages.row.name', { ...params, count: 2 })).toContain('2 need review')
  })

  it('counts the param an entry names with `select`, not `count`', () => {
    expect(t('editor.readout.reviewPending', { total: 0 })).toBe('Nothing to review')
    expect(t('editor.readout.reviewPending', { total: 1 })).toBe('1 to review')
    expect(t('editor.readout.reviewPending', { total: 5 })).toBe('5 to review')
  })

  it('falls back to `other` when the counted param is absent or not a number', () => {
    expect(selectForm({ one: 'one', other: 'other' }, {})).toBe('other')
    expect(selectForm({ one: 'one', other: 'other' }, { count: NaN })).toBe('other')
    expect(selectForm({ zero: 'zero', other: 'other' }, {})).toBe('other')
  })

  it('only takes the zero form at exactly zero, and only when one is declared', () => {
    expect(selectForm({ zero: 'zero', one: 'one', other: 'other' }, { count: 0 })).toBe('zero')
    expect(selectForm({ one: 'one', other: 'other' }, { count: 0 })).toBe('other')
  })
})

describe('a missing key', () => {
  it('renders the key itself and shouts, rather than rendering undefined', () => {
    const spy = vi.spyOn(console, 'error').mockImplementation(() => {})
    expect(t('masks.value.thereIsNoSuchKey')).toBe('[masks.value.thereIsNoSuchKey]')
    expect(spy).toHaveBeenCalledOnce()
    // Once per key, not once per render: a missing key in a list is one bug.
    t('masks.value.thereIsNoSuchKey')
    expect(spy).toHaveBeenCalledOnce()
    spy.mockRestore()
  })

  it('degrades to legible words in a build', () => {
    expect(humanise('pages.status.cleanedNeedsReview')).toBe('Cleaned needs review')
  })
})

describe('flatten', () => {
  it('joins nested names with dots', () => {
    expect(flatten({ a: { b: { c: 'x' } } })).toEqual({ 'a.b.c': 'x' })
  })

  it('treats a plural-forms object as a leaf, not a branch', () => {
    const forms = { one: 'x', other: 'y' }
    expect(flatten({ a: { b: forms } })).toEqual({ 'a.b': forms })
  })
})

describe('the key scanner', () => {
  it('finds a key in a t() call', () => {
    expect([...keysIn("t('masks.action.delete')")]).toEqual(['masks.action.delete'])
  })

  it('ignores a string that is not in a known namespace', () => {
    expect([...keysIn("import x from './thing.js'")]).toEqual([])
  })

  it('does not count a key named in a comment', () => {
    expect([...keysIn("/** see `'masks.action.delete'` */")]).toEqual([])
    expect([...keysIn("// 'masks.action.delete'")]).toEqual([])
    expect([...keysIn("<!-- 'masks.action.delete' -->")]).toEqual([])
  })

  it('keeps a URL inside a string intact', () => {
    expect(stripComments("const u = 'https://x/y'")).toContain('https://x/y')
  })

  it('finds a key in any of the three quotes', () => {
    expect([...keysIn('t("masks.action.delete")')]).toEqual(['masks.action.delete'])
    expect([...keysIn('t(`masks.action.delete`)')]).toEqual(['masks.action.delete'])
    // An interpolated template has no literal to find, by construction.
    expect([...keysIn('t(`masks.hint.${id}`)')]).toEqual([])
  })

  it('does not mistake a shortcut id for a key - in shortcuts.js only', () => {
    const source = "{ id: 'app.export', labelKey: 'shortcuts.app.export' }"
    expect([...keysIn(source, 'lib/shortcuts.js')]).toEqual(['shortcuts.app.export'])
    // Anywhere else, `id` is just a property and its value is not discarded.
    expect([...keysIn(source, 'lib/editor/tools.js')]).toEqual([
      'app.export',
      'shortcuts.app.export',
    ])
    expect(stripIds("id: 'app.export'")).not.toContain('app.export')
  })
})
