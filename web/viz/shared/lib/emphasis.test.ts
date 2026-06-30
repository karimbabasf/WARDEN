import { describe, it, expect } from 'vitest'
import {
  severityBucket,
  matchesFilter,
  targetDim,
  EmphasisFilter,
  EmphasisNode,
} from './emphasis'

describe('severityBucket', () => {
  it('maps 1 to low', () => {
    expect(severityBucket(1)).toBe('low')
  })
  it('maps 2 to low', () => {
    expect(severityBucket(2)).toBe('low')
  })
  it('maps 3 to med', () => {
    expect(severityBucket(3)).toBe('med')
  })
  it('maps 4 to high', () => {
    expect(severityBucket(4)).toBe('high')
  })
  it('maps 5 to crit', () => {
    expect(severityBucket(5)).toBe('crit')
  })
  it('maps 6 to crit', () => {
    expect(severityBucket(6)).toBe('crit')
  })
})

describe('matchesFilter', () => {
  describe('null filter', () => {
    it('matches any node with null filter', () => {
      expect(matchesFilter({ severity: 3 }, null)).toBe(true)
      expect(matchesFilter({ harness: 'claude' }, null)).toBe(true)
      expect(matchesFilter({}, null)).toBe(true)
    })
  })

  describe('severity filter', () => {
    it('matches node with matching severity bucket', () => {
      expect(matchesFilter({ severity: 2 }, { kind: 'severity', bucket: 'low' })).toBe(true)
      expect(matchesFilter({ severity: 3 }, { kind: 'severity', bucket: 'med' })).toBe(true)
      expect(matchesFilter({ severity: 4 }, { kind: 'severity', bucket: 'high' })).toBe(true)
      expect(matchesFilter({ severity: 5 }, { kind: 'severity', bucket: 'crit' })).toBe(true)
    })

    it('does not match node with non-matching severity bucket', () => {
      expect(matchesFilter({ severity: 1 }, { kind: 'severity', bucket: 'med' })).toBe(false)
      expect(matchesFilter({ severity: 3 }, { kind: 'severity', bucket: 'low' })).toBe(false)
      expect(matchesFilter({ severity: 4 }, { kind: 'severity', bucket: 'low' })).toBe(false)
    })

    it('handles nodes with missing severity', () => {
      expect(matchesFilter({ harness: 'claude' }, { kind: 'severity', bucket: 'low' })).toBe(
        false
      )
      expect(matchesFilter({}, { kind: 'severity', bucket: 'med' })).toBe(false)
    })
  })

  describe('harness filter', () => {
    it('matches node with same harness', () => {
      expect(matchesFilter({ harness: 'claude' }, { kind: 'harness', harness: 'claude' })).toBe(
        true
      )
      expect(matchesFilter({ harness: 'codex' }, { kind: 'harness', harness: 'codex' })).toBe(
        true
      )
    })

    it('does not match node with different harness', () => {
      expect(matchesFilter({ harness: 'claude' }, { kind: 'harness', harness: 'codex' })).toBe(
        false
      )
      expect(matchesFilter({ harness: 'codex' }, { kind: 'harness', harness: 'claude' })).toBe(
        false
      )
    })

    it('handles nodes with null or missing harness', () => {
      expect(matchesFilter({ harness: null }, { kind: 'harness', harness: 'claude' })).toBe(false)
      expect(matchesFilter({}, { kind: 'harness', harness: 'claude' })).toBe(false)
    })
  })
})

describe('targetDim', () => {
  it('returns 0 (not dimmed) when filter is null', () => {
    expect(targetDim({ severity: 2 }, null)).toBe(0)
    expect(targetDim({ harness: 'claude' }, null)).toBe(0)
    expect(targetDim({}, null)).toBe(0)
  })

  it('returns 0 (not dimmed) when node matches the filter', () => {
    expect(targetDim({ severity: 2 }, { kind: 'severity', bucket: 'low' })).toBe(0)
    expect(targetDim({ harness: 'claude' }, { kind: 'harness', harness: 'claude' })).toBe(0)
  })

  it('returns 1 (dimmed) when node does not match the filter', () => {
    expect(targetDim({ severity: 3 }, { kind: 'severity', bucket: 'low' })).toBe(1)
    expect(targetDim({ harness: 'codex' }, { kind: 'harness', harness: 'claude' })).toBe(1)
    expect(targetDim({}, { kind: 'severity', bucket: 'low' })).toBe(1)
  })

  it('handles combinations of severity and harness', () => {
    const node: EmphasisNode = { severity: 5, harness: 'claude' }
    expect(targetDim(node, { kind: 'severity', bucket: 'crit' })).toBe(0)
    expect(targetDim(node, { kind: 'severity', bucket: 'low' })).toBe(1)
    expect(targetDim(node, { kind: 'harness', harness: 'claude' })).toBe(0)
    expect(targetDim(node, { kind: 'harness', harness: 'codex' })).toBe(1)
  })
})
