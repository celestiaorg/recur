import { describe, expect, it } from 'vitest'
import {
  isReenterPhaseTool,
  parseReenterPhaseID,
  reenterPhaseFailed,
  reenterPhaseLabel,
} from './reenter-phase-tool'

describe('isReenterPhaseTool', () => {
  it('matches Recur MCP and bare names', () => {
    expect(isReenterPhaseTool('mcp__recur__reenter_phase')).toBe(true)
    expect(isReenterPhaseTool('reenter_phase')).toBe(true)
    expect(isReenterPhaseTool('mcp__recur__run_phase')).toBe(false)
    expect(isReenterPhaseTool(undefined)).toBe(false)
  })
})

describe('parseReenterPhaseID', () => {
  it('reads phase_id and ignores other args', () => {
    expect(parseReenterPhaseID({ phase_id: 'converge' })).toBe('converge')
    expect(parseReenterPhaseID({ phaseId: 'review' })).toBe('review')
    expect(parseReenterPhaseID({ phase_id: '  plan  ' })).toBe('plan')
    expect(parseReenterPhaseID({})).toBe('')
    expect(parseReenterPhaseID(null)).toBe('')
  })
})

describe('reenterPhaseLabel', () => {
  it('names the phase without dumping JSON or the engine ack', () => {
    expect(reenterPhaseLabel({ pending: true, failed: false, phase: 'converge' })).toBe(
      'Re-entering converge',
    )
    expect(reenterPhaseLabel({ pending: false, failed: false, phase: 'converge' })).toBe(
      'Re-entered converge',
    )
    expect(reenterPhaseLabel({ pending: false, failed: true, phase: 'converge' })).toBe(
      'Re-enter failed',
    )
    expect(reenterPhaseLabel({ pending: false, failed: false, phase: '' })).toBe('Re-entered phase')
    const label = reenterPhaseLabel({ pending: false, failed: false, phase: 'converge' })
    expect(label).not.toContain('{')
    expect(label).not.toContain('phase_id')
    expect(label).not.toMatch(/Loop restarting/)
  })
})

describe('reenterPhaseFailed', () => {
  it('treats engine refusals as failed, not the restart ack', () => {
    expect(reenterPhaseFailed('Loop restarting — it continues from the current worktree, steered by the recorded plan.')).toBe(false)
    expect(reenterPhaseFailed('Refused: reenter_phase is idle only.')).toBe(true)
    expect(reenterPhaseFailed('Refused: converge is not on this loop\'s reenter list.')).toBe(true)
    expect(reenterPhaseFailed('reenter_phase: phase_id is required')).toBe(true)
    expect(reenterPhaseFailed('', { error: true })).toBe(true)
  })
})
