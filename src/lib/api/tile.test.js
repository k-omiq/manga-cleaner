import { afterEach, describe, expect, it } from 'vitest'

import {
  fold,
  pageVersion,
  proxyPlan,
  tileOrigin,
  tileUrl,
  PROXY_SHORT_EDGE,
  PROXY_TILE_LONG_EDGE,
  TILE_SCHEME,
} from './tile.js'

/**
 * Tauri's own `convertFileSrc`, verbatim from `tauri/scripts/core.js`, with the
 * platform as a parameter.
 *
 * Copied rather than described because the whole point of asking Tauri for the
 * origin is that it is *Tauri's* rule and not a guess restated here - a test
 * that reimplemented it loosely could pass against a rule the webview does not
 * follow. Windows and Android have never been executed on this machine;
 * this is what the shipped script does on them, not a measurement of one.
 *
 * @param {'macos'|'linux'|'windows'|'android'} osName
 * @param {string} [protocolScheme]
 */
function tauriWindow(osName, protocolScheme = 'http') {
  return {
    convertFileSrc(filePath, protocol = 'asset') {
      const path = encodeURIComponent(filePath)
      return osName === 'windows' || osName === 'android'
        ? `${protocolScheme}://${protocol}.localhost/${path}`
        : `${protocol}://localhost/${path}`
    },
  }
}

/** @param {'macos'|'linux'|'windows'|'android'|null} osName */
function inside(osName, protocolScheme) {
  if (osName === null) delete globalThis.__TAURI_INTERNALS__
  else globalThis.__TAURI_INTERNALS__ = tauriWindow(osName, protocolScheme)
}

/** A page in the shape the adapter sends, with `count` cleaned regions. */
function aPage({ id = 'ch-1', index = 0, sha = 'aa11', masks = [] } = {}) {
  return {
    id: `${id}-p${index}`,
    chapterId: id,
    index,
    sourceSha: sha,
    regions: masks.map((mask, i) => ({
      id: `r${i}`,
      mask: mask && {
        id: mask.id,
        sequence: mask.sequence,
        provenance: {
          mask_sha256: mask.sha,
          created: mask.created,
          engine: mask.engine,
        },
      },
    })),
  }
}

afterEach(() => inside(null))

describe('the tile origin', () => {
  /**
   * `tile://localhost/` on macOS, iOS and Linux; `http://tile.localhost/` on
   * Windows and Android. Both are whitelisted in
   * `src-tauri/tauri.conf.json`'s `img-src`, and if this ever answered a third
   * thing the CSP would block it.
   */
  it('is the one Tauri chose for this platform', () => {
    inside('macos')
    expect(tileOrigin()).toBe('tile://localhost/')
    inside('linux')
    expect(tileOrigin()).toBe('tile://localhost/')
    inside('windows')
    expect(tileOrigin()).toBe('http://tile.localhost/')
    inside('android')
    expect(tileOrigin()).toBe('http://tile.localhost/')
  })

  /**
   * A window built with `use_https_scheme` serves the same protocol over
   * `https://tile.localhost/`. Nothing in this repository sets it, and this is
   * here so that turning it on is a failing CSP rather than a silent one: the
   * origin follows the window, and `tauri.conf.json` would have to follow it
   * too.
   */
  it('follows the window scheme rather than assuming http', () => {
    inside('windows', 'https')
    expect(tileOrigin()).toBe('https://tile.localhost/')
  })

  it('is null outside a Tauri window, where there is no protocol at all', () => {
    inside(null)
    expect(tileOrigin()).toBeNull()
    expect(tileUrl(aPage(), 'source')).toBeNull()
  })
})

describe('a tile URL', () => {
  it('is the shape the protocol parses', () => {
    inside('macos')
    const url = tileUrl(aPage({ id: 'ch-1', index: 7 }), 'cleaned')
    // The path `src-tauri/src/tile.rs#parse` splits into three, and the query
    // it deliberately does not read.
    expect(url).toMatch(/^tile:\/\/localhost\/ch-1\/7\/cleaned\?v=[0-9a-f]{16}$/)
  })

  it('carries the same path under the Windows origin', () => {
    inside('windows')
    const url = tileUrl(aPage({ id: 'ch-1', index: 7 }), 'cleaned')
    expect(url?.startsWith('http://tile.localhost/ch-1/7/cleaned?v=')).toBe(true)
  })

  /** The Rust side percent-decodes, so this side must percent-encode. */
  it('escapes a chapter id that is not a bare path segment', () => {
    inside('macos')
    expect(tileUrl(aPage({ id: 'ch 1/2' }), 'source')).toContain('/ch%201%2F2/0/source?v=')
  })

  it('is null for a page with no chapter or no index', () => {
    inside('macos')
    expect(tileUrl(null, 'source')).toBeNull()
    expect(tileUrl({ ...aPage(), chapterId: '' }, 'source')).toBeNull()
    expect(tileUrl({ ...aPage(), index: undefined }, 'source')).toBeNull()
  })

  it('names the scheme the Rust module registers', () => {
    expect(TILE_SCHEME).toBe('tile')
  })

  /** The fourth segment `src-tauri/src/tile.rs#parse` reads. */
  it('names a proxy tile when it is given one', () => {
    inside('macos')
    const page = aPage({ id: 'ch-1', index: 7 })
    expect(tileUrl(page, 'cleaned', 0)).toMatch(
      /^tile:\/\/localhost\/ch-1\/7\/cleaned\/0\?v=[0-9a-f]{16}$/,
    )
    expect(tileUrl(page, 'cleaned', 3)).toContain('/ch-1/7/cleaned/3?v=')
    // No tile is the page at its own resolution, which is a different URL.
    expect(tileUrl(page, 'cleaned')).toContain('/ch-1/7/cleaned?v=')
  })

  it('refuses a tile index that is not one, rather than asking for tile 0', () => {
    inside('macos')
    const page = aPage()
    expect(tileUrl(page, 'source', -1)).toBeNull()
    expect(tileUrl(page, 'source', 1.5)).toBeNull()
    expect(tileUrl(page, 'source', /** @type {any} */ ('0'))).toBeNull()
  })
})

/**
 * These numbers are also asserted, in the same words, by
 * `cleaner_core::image::proxy`'s own tests - the two implementations are
 * separate on purpose (see the module header) and this is what pins them
 * together.
 */
describe('the proxy plan', () => {
  it('caps the short edge and leaves the long one to the tiles', () => {
    const plan = proxyPlan({ width: 2400, height: 3600 })
    expect(plan.width).toBe(PROXY_SHORT_EDGE)
    expect(plan.height).toBe(1536)
    expect(plan.vertical).toBe(true)
  })

  it('does not render a webtoon segment as a sliver', () => {
    // The failure rule 7 names outright: cap the long edge and this page is
    // 40×1024.
    const plan = proxyPlan({ width: 800, height: 20_000 })
    expect(plan.width).toBe(800)
    expect(plan.height).toBe(20_000)
    expect(plan.tiles).toHaveLength(10)
  })

  it('cuts the long axis into tiles that cover the page exactly once', () => {
    const plan = proxyPlan({ width: 800, height: 5000 })
    expect(plan.tiles).toHaveLength(3)
    let covered = 0
    for (const [at, tile] of plan.tiles.entries()) {
      expect(tile.index).toBe(at)
      expect(tile.offset).toBeCloseTo(covered, 10)
      covered += tile.extent
    }
    expect(covered).toBeCloseTo(100, 10)
    // The last band is the short one.
    expect(plan.tiles[2].extent).toBeLessThan(plan.tiles[0].extent)
  })

  it('tiles the long axis whichever axis that is', () => {
    const plan = proxyPlan({ width: 6000, height: 1200 })
    expect(plan.vertical).toBe(false)
    expect(plan.height).toBe(PROXY_SHORT_EDGE)
    expect(plan.width).toBe(5120)
    expect(plan.tiles).toHaveLength(3)
  })

  it('is a cap and never an enlargement', () => {
    const plan = proxyPlan({ width: 400, height: 600 })
    expect(plan.width).toBe(400)
    expect(plan.height).toBe(600)
    expect(plan.tiles).toHaveLength(1)
  })

  /**
   * Guessing one whole tile for an undeclared page is a wrong picture
   * rendered with confidence, once `PageArtwork` stretches whatever
   * that one tile turns out to be over 100% of the sheet. Refusing - no
   * tiles - instead sends the component down the same stand-in path it
   * already takes when there is no protocol to ask at all.
   */
  it('refuses rather than guesses one tile for a page that has not said how big it is', () => {
    for (const page of [null, undefined, {}, { width: 0, height: 0 }, { width: 800 }]) {
      const plan = proxyPlan(page)
      expect(plan.tiles).toHaveLength(0)
      expect(plan.width).toBe(0)
      expect(plan.height).toBe(0)
    }
  })

  it('agrees with the constants the Rust half is compiled with', () => {
    expect(PROXY_SHORT_EDGE).toBe(1024)
    expect(PROXY_TILE_LONG_EDGE).toBe(2048)
  })
})

describe('the version token', () => {
  /**
   * A hash decides staleness. The token is folded from content
   * hashes and from nothing else, so calling it twice on an unchanged page is
   * the same URL - which is what makes the response cacheable at all.
   */
  it('is stable for an unchanged page', () => {
    const page = aPage({ masks: [{ id: 'm1', sequence: 1, sha: 'ff' }] })
    expect(pageVersion(page, 'cleaned')).toBe(pageVersion(page, 'cleaned'))
    expect(pageVersion(page, 'source')).toBe(pageVersion(aPage(), 'source'))
  })

  it('changes when the source does', () => {
    expect(pageVersion(aPage({ sha: 'aa' }), 'source')).not.toBe(
      pageVersion(aPage({ sha: 'bb' }), 'source'),
    )
  })

  /** Every way a patch set can move: a new mask, a re-run, a deletion. */
  it('changes when the patch set does, and only for the cleaned variant', () => {
    const none = aPage()
    const one = aPage({ masks: [{ id: 'm1', sequence: 1, sha: 'ff' }] })
    const rerun = aPage({ masks: [{ id: 'm1', sequence: 2, sha: 'ee' }] })
    const two = aPage({
      masks: [
        { id: 'm1', sequence: 1, sha: 'ff' },
        { id: 'm2', sequence: 1, sha: 'dd' },
      ],
    })

    const cleaned = [none, one, rerun, two].map((page) => pageVersion(page, 'cleaned'))
    expect(new Set(cleaned).size).toBe(4)

    // The source page is the same file throughout: a clean run must not
    // invalidate the tile of the page it started from, or the wipe re-fetches
    // the original on every region.
    const sources = [none, one, rerun, two].map((page) => pageVersion(page, 'source'))
    expect(new Set(sources).size).toBe(1)
  })

  /**
   * Re-running inpainting produces new pixels while the mask geometry (and sequence/id)
   * can remain identical. The token must distinguish different runs via created timestamp or engine.
   */
  it('changes when inpainting is rerun with identical mask geometry but different timestamp or engine', () => {
    const first = aPage({
      masks: [{ id: 'r0-m1', sequence: 1, sha: 'ff', created: '2026-01-01T00:00:00Z', engine: 'lama' }],
    })
    const rerun = aPage({
      masks: [{ id: 'r0-m1', sequence: 1, sha: 'ff', created: '2026-01-01T00:01:00Z', engine: 'lama' }],
    })
    const diffEngine = aPage({
      masks: [{ id: 'r0-m1', sequence: 1, sha: 'ff', created: '2026-01-01T00:00:00Z', engine: 'denoise' }],
    })
    expect(pageVersion(first, 'cleaned')).not.toBe(pageVersion(rerun, 'cleaned'))
    expect(pageVersion(first, 'cleaned')).not.toBe(pageVersion(diffEngine, 'cleaned'))
  })

  /**
   * The backend is free to return a page's regions in any order; that is not a
   * change to the page.
   */
  it('does not move when the regions are merely reordered', () => {
    const forwards = aPage({
      masks: [
        { id: 'm1', sequence: 1, sha: 'ff' },
        { id: 'm2', sequence: 1, sha: 'dd' },
      ],
    })
    const backwards = aPage({
      masks: [
        { id: 'm2', sequence: 1, sha: 'dd' },
        { id: 'm1', sequence: 1, sha: 'ff' },
      ],
    })
    expect(pageVersion(forwards, 'cleaned')).toBe(pageVersion(backwards, 'cleaned'))
  })

  /** A region with no mask is a region nothing was applied to. */
  it('ignores regions that were never cleaned', () => {
    const page = aPage({ masks: [null, { id: 'm1', sequence: 1, sha: 'ff' }, null] })
    const only = aPage({ masks: [{ id: 'm1', sequence: 1, sha: 'ff' }] })
    expect(pageVersion(page, 'cleaned')).toBe(pageVersion(only, 'cleaned'))
  })

  /**
   * `Cache-Control: immutable` pins a tile for a year, so the token has to
   * cover everything that decides what a tile *is* - including the provisional
   * geometry constants. Asserted by construction rather than by
   * mutating a module constant: the token is built from them, so a change to
   * either moves it.
   */
  it('covers the tile geometry as well as the content', () => {
    const page = aPage()
    const token = pageVersion(page, 'source')
    expect(token).toBe(fold(`${PROXY_SHORT_EDGE}x${PROXY_TILE_LONG_EDGE}|${page.sourceSha}`))
    expect(token).not.toBe(fold(`|${page.sourceSha}`))
  })

  it('is sixteen hex characters, so it is a URL-safe cache key', () => {
    expect(fold('')).toMatch(/^[0-9a-f]{16}$/)
    expect(fold('a')).toMatch(/^[0-9a-f]{16}$/)
    expect(fold('a')).not.toBe(fold('b'))
    // Non-ASCII must not collapse: a chapter name reaches this string.
    expect(fold('第1話')).not.toBe(fold('第2話'))
  })
})
