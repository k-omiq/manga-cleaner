/**
 * The Settings dialog's one piece of arithmetic: whether a download this window
 * is hearing about is one its own list already knows about.
 *
 * No DOM and no mount. `model-progress` is a process-wide stream and
 * `listModels` is a snapshot, so the question "is this snapshot behind the
 * world" is decided by comparing the two - and that comparison is a pure
 * function this file imports from the component's module script. What is left
 * around it in the dialog is a timer and a call, which mounting a 520px dialog
 * to assert would be a poor trade.
 */

import { describe, expect, it } from 'vitest'

import { isStrangerDownload } from './SettingsDialog.svelte'

/**
 * A catalogue of two rows and a runtime, with `downloading` where asked.
 *
 * Both ids are real ones from `MODELS` in `src-tauri/src/weights.rs`. The
 * second used to be the preview engine's weight, which left the catalogue
 * with the engine - this function supplies its own rows, so nothing failed,
 * and a fixture naming a weight that no longer ships is a fixture that has
 * stopped describing what it is about.
 */
function view({ downloading = [], runtime = false } = {}) {
  return {
    models: [
      { id: 'inpainter', downloading: downloading.includes('inpainter') },
      { id: 'balloonDetector', downloading: downloading.includes('balloonDetector') },
    ],
    runtime: { downloading: runtime },
  }
}

describe('a download this window did not start', () => {
  it('is a stranger when the row does not know it is downloading', () => {
    // The second window's case: its rows were drawn before the first window
    // pressed anything, so the row says "not installed" while the bytes arrive.
    expect(isStrangerDownload(view(), 'balloonDetector')).toBe(true)
  })

  it('is not a stranger when the row already says it is downloading', () => {
    // This window's own press: `listModels` after `downloadModel` already
    // reported it, and refreshing again on every 4 MiB would be a poll.
    expect(isStrangerDownload(view({ downloading: ['balloonDetector'] }), 'balloonDetector')).toBe(
      false,
    )
    // And another row downloading is not this row's answer.
    expect(isStrangerDownload(view({ downloading: ['inpainter'] }), 'balloonDetector')).toBe(true)
  })

  it('reads the runtime from its own row rather than from the catalogue', () => {
    // The runtime is not a `models` entry - it is an archive - so an id that
    // matches nothing would otherwise always look like a stranger.
    expect(isStrangerDownload(view({ runtime: true }), 'runtime')).toBe(false)
    expect(isStrangerDownload(view({ runtime: false }), 'runtime')).toBe(true)
  })

  it('treats an id with no row at all as a stranger', () => {
    // A catalogue older than the backend: a weight added since this snapshot.
    expect(isStrangerDownload(view(), 'somethingNewer')).toBe(true)
  })

  it('asks for nothing before the first answer has arrived', () => {
    // The mount's own `listModels` is already in flight; a second call would
    // answer the same question twice and race it.
    expect(isStrangerDownload(null, 'balloonDetector')).toBe(false)
    expect(isStrangerDownload(null, 'runtime')).toBe(false)
  })
})
