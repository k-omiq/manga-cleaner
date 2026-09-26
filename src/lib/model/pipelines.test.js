/**
 * The pure half of the capability graph: what a set of choices needs, what a
 * retired choice meant, and what a removal stops. The mounted halves are
 * `SettingsDialog.models.dom.test.js` and `FirstLaunchDialog.dom.test.js`.
 */

import { describe, expect, it } from 'vitest'
import {
  ALL_TEXT_POLICY,
  CAPABILITIES,
  DETECTORS,
  LEGACY_POLICY,
  MODELS,
  OCR_FILES,
  SCRIPT_GATE_FILES,
  detectorsFor,
  filesFor,
  migrateDetectorChoice,
  model,
  modelOfFile,
  removalImpact,
  rescueRuns,
  runtimeState,
  usedNow,
  workflowNeeds,
} from './pipelines.js'

const ALL = { ja: 'ctd-rtdetr', zh: 'ctd-rtdetr', ko: 'ctd-rtdetr' }
const NONE = { ja: null, zh: null, ko: null }

describe('the per-language detector choices', () => {
  it('offer no OCR variant: the rescue is its own switch', () => {
    for (const language of ['ja', 'zh', 'ko']) {
      expect(detectorsFor(language).map((engine) => engine.id)).toEqual(['ctd-rtdetr'])
    }
    expect(DETECTORS.flatMap((engine) => engine.files)).not.toEqual(expect.arrayContaining([...OCR_FILES]))
  })

  it('never carry the script gate or OCR inside a detector', () => {
    for (const engine of DETECTORS) {
      for (const file of [...SCRIPT_GATE_FILES, ...OCR_FILES]) expect(engine.files).not.toContain(file)
    }
  })

  it('read a retired ctd-rtdetr-ocr row as the plain detector with rescue on, for Japanese only', () => {
    expect(migrateDetectorChoice('ja', 'ctd-rtdetr-ocr')).toEqual({ detector: 'ctd-rtdetr', ocrRescue: true })
    expect(migrateDetectorChoice('ko', 'ctd-rtdetr-ocr')).toEqual({ detector: undefined, ocrRescue: false })
    expect(migrateDetectorChoice('ja', 'ctd-rtdetr')).toEqual({ detector: 'ctd-rtdetr', ocrRescue: false })
    expect(migrateDetectorChoice('ja', null)).toEqual({ detector: null, ocrRescue: false })
    expect(migrateDetectorChoice('ja', 'nonsense')).toEqual({ detector: undefined, ocrRescue: false })
    expect(migrateDetectorChoice('ja', 42)).toEqual({ detector: undefined, ocrRescue: false })
  })
})

describe('what legacy filtering downloads', () => {
  it('is the detector pair and the script gate, in download order, and no OCR by default', () => {
    expect(filesFor(ALL, { 'lama-manga': true })).toEqual([
      'textDetector', 'balloonDetector', 'scriptGate', 'scriptGateLabels', 'inpainter',
    ])
    expect(filesFor(ALL, {}, { textPolicy: LEGACY_POLICY, ocrRescue: false })).not.toContain('ocrEncoder')
  })

  it('adds the three OCR files only when the rescue is on and Japanese is cleaned', () => {
    expect(filesFor(ALL, {}, { ocrRescue: true })).toEqual([
      'textDetector', 'balloonDetector', 'scriptGate', 'scriptGateLabels', ...OCR_FILES,
    ])
    expect(filesFor({ ...ALL, ja: null }, {}, { ocrRescue: true })).not.toContain('ocrEncoder')
    expect(rescueRuns({ ...ALL, ja: null }, true)).toBe(false)
    expect(rescueRuns(ALL, true)).toBe(true)
    expect(rescueRuns(ALL, false)).toBe(false)
  })

  it('still honours a stored retired row until it is migrated', () => {
    expect(filesFor({ ...ALL, ja: 'ctd-rtdetr-ocr' }, {})).toEqual(expect.arrayContaining([...OCR_FILES]))
  })

  it('needs nothing for detection once every language is skipped, rescue or not', () => {
    expect(filesFor(NONE, { 'lama-manga': true }, { ocrRescue: true })).toEqual(['inpainter'])
    expect(workflowNeeds({ textPolicy: LEGACY_POLICY, detection: NONE, ocrRescue: true })).toEqual([])
  })
})

describe('what all-text review downloads', () => {
  it('is the speech bubble finder alone: never the gate labels, never any manga-ocr file', () => {
    for (const ocrRescue of [false, true]) {
      for (const detection of [ALL, NONE, { ...ALL, ja: 'ctd-rtdetr-ocr' }]) {
        const files = filesFor(detection, { 'lama-manga': true }, { textPolicy: ALL_TEXT_POLICY, ocrRescue })
        expect(files).toEqual(['balloonDetector', 'inpainter'])
      }
    }
  })

  it('needs the SAM-TS-L graphs as an import, not a download', () => {
    expect(workflowNeeds({ textPolicy: ALL_TEXT_POLICY })).toEqual(['rtSmall', 'samTs'])
    expect(model('samTs')?.source).toBe('import')
    expect(model('rtFull')?.source).toBe('import')
    expect(model('samTs')?.files).toEqual([])
  })
})

describe('the engine runtime a workflow runs on', () => {
  const RUNTIME = { installed: true, available: true, downloading: false }

  it('decides nothing for a workflow that runs nothing', () => {
    const needs = workflowNeeds({ detection: NONE })
    expect(runtimeState({ ...RUNTIME, installed: false }, needs)).toBe('notNeeded')
    expect(runtimeState(RUNTIME, needs, 'failed')).toBe('notNeeded')
  })

  it('is needed by every workflow that runs a model, legacy or all-text', () => {
    for (const choices of [{ detection: ALL }, { textPolicy: ALL_TEXT_POLICY }]) {
      const needs = workflowNeeds(choices)
      expect(runtimeState(RUNTIME, needs, 'loaded')).toBe('installed')
      expect(runtimeState({ ...RUNTIME, installed: false }, needs, 'loaded')).toBe('missing')
    }
  })

  it('tells a download in flight and a computer with no build apart from plain missing', () => {
    const needs = workflowNeeds({ detection: ALL })
    expect(runtimeState({ ...RUNTIME, installed: false, downloading: true }, needs)).toBe('downloading')
    expect(runtimeState({ ...RUNTIME, installed: false, available: false }, needs)).toBe('unavailable')
    // An installed runtime runs whether or not a newer build is offered.
    expect(runtimeState({ ...RUNTIME, available: false }, needs, 'loaded')).toBe('installed')
    // No row at all is not a runtime that is here.
    expect(runtimeState(null, needs)).toBe('missing')
  })

  // The catalogue's `installed` is a file found. A run also loads it, and a
  // CUDA build without CUDA, or a quarantined library, is found and refused.
  it('counts an installed runtime as installed only once it has loaded', () => {
    const needs = workflowNeeds({ detection: ALL })
    expect(runtimeState(RUNTIME, needs, 'failed')).toBe('unloadable')
    expect(runtimeState(RUNTIME, needs, 'checking')).toBe('checking')
    expect(runtimeState(RUNTIME, needs, 'unchecked')).toBe('unchecked')
    // No answer at all is a question not yet asked, never a pass.
    expect(runtimeState(RUNTIME, needs)).toBe('checking')
    // A load answer about a runtime that is not here changes nothing.
    expect(runtimeState({ ...RUNTIME, installed: false }, needs, 'failed')).toBe('missing')
    expect(runtimeState({ ...RUNTIME, installed: false, downloading: true }, needs, 'failed')).toBe('downloading')
  })
})

describe('the capability graph', () => {
  it('draws five sections in the order the plan names them, every model once', () => {
    expect(CAPABILITIES.map((section) => section.id)).toEqual(['findRegions', 'shapeMask', 'sfx', 'japanese', 'rebuild'])
    const drawn = CAPABILITIES.flatMap((section) => section.models)
    expect([...drawn].sort()).toEqual(MODELS.map((entry) => entry.id).sort())
  })

  it('claims every catalogue file exactly once', () => {
    const files = MODELS.flatMap((entry) => entry.files)
    expect(new Set(files).size).toBe(files.length)
    expect(modelOfFile('scriptGateLabels')?.id).toBe('scriptGate')
    expect(modelOfFile('ocrVocab')?.id).toBe('mangaOcr')
    expect(modelOfFile('somethingNew')).toBeNull()
  })

  it('keeps COO out of every workflow: excluded, nothing to remove', () => {
    expect(model('coo')?.source).toBe('excluded')
    expect(removalImpact('coo')).toBeNull()
    expect(workflowNeeds({ textPolicy: ALL_TEXT_POLICY })).not.toContain('coo')
  })
})

describe('what a removal stops', () => {
  it('names legacy cleaning and the review small profile for the small RT-DETR', () => {
    expect(removalImpact('rtSmall')?.disables).toEqual(['legacy', 'reviewSmall'])
    expect(removalImpact('balloonDetector')?.model.id).toBe('rtSmall')
  })

  it('answers for the whole group from any member file', () => {
    expect(removalImpact('scriptGateLabels')).toEqual({
      model: model('scriptGate'),
      files: ['scriptGate', 'scriptGateLabels'],
      disables: ['legacy'],
    })
    expect(removalImpact('ocrDecoder')?.files).toEqual([...OCR_FILES])
  })

  it('never includes the shared RT-DETR weights in another group', () => {
    for (const id of ['scriptGate', 'mangaOcr', 'ctd', 'samTs', 'rtFull', 'lama']) {
      expect(removalImpact(id)?.files).not.toContain('balloonDetector')
    }
  })

  it('knows when the model is in use by the cleaning in force', () => {
    expect(usedNow('rtSmall', { detection: ALL })).toBe(true)
    expect(usedNow('mangaOcr', { detection: ALL, ocrRescue: false })).toBe(false)
    expect(usedNow('scriptGate', { textPolicy: ALL_TEXT_POLICY, detection: ALL })).toBe(false)
    expect(usedNow('samTs', { textPolicy: ALL_TEXT_POLICY })).toBe(true)
  })
})
