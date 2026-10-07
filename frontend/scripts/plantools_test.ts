// Run: npx --yes tsx scripts/plantools_test.ts
import assert from 'node:assert/strict'
import {
  isSubmitPlanTool,
  isUpdatePlanStepTool,
  isEditPlanTool,
  submitPlanLabel,
  updatePlanStepLabel,
  editPlanLabel,
  updatePlanStepMeta,
  parseSubmitPlanSteps,
  parseSubmitPlanBody,
  parseSubmitPlanAttachments,
  parseUpdatePlanStepTitle,
  parseEditPlan,
  prosePreview,
} from '../src/lib/plantools.ts'

assert.equal(isSubmitPlanTool('mcp__recur__submit_plan'), true)
assert.equal(isUpdatePlanStepTool('mcp__recur__update_plan_step'), true)
assert.equal(isSubmitPlanTool('mcp__recur__plan'), false)

assert.equal(submitPlanLabel(true, 4), 'Submitting plan')
assert.equal(submitPlanLabel(false, 4), 'Submitted plan · 4 steps')
assert.equal(submitPlanLabel(false, 1), 'Submitted plan · 1 step')
assert.equal(updatePlanStepLabel(false), 'Updated plan step')

assert.equal(updatePlanStepMeta('S2', 'done', ''), 'S2 → done')
assert.equal(updatePlanStepMeta('S2', 'done', 'shipped'), 'S2 → done · shipped')
assert.ok(!updatePlanStepMeta('S2', 'done', '').includes('{step_id'))

const steps = parseSubmitPlanSteps([
  'Examine AgentChat toolCard',
  '  ',
  { thought: 'Wire CSS', status: 'todo', step_id: 'S9' },
])
assert.equal(steps.length, 2)
assert.equal(steps[0].id, 'S1')
assert.equal(steps[0].title, 'Examine AgentChat toolCard')
assert.equal(steps[1].id, 'S9')
assert.equal(steps[1].title, 'Wire CSS')

const title = parseUpdatePlanStepTitle(
  'Step S3 (Ensure the persisted transcript / ChatItem mapping also renders) → done.',
)
assert.equal(title, 'Ensure the persisted transcript / ChatItem mapping also renders')
assert.equal(parseUpdatePlanStepTitle('Refused: no plan step with id "S9".'), '')

assert.equal(isEditPlanTool('mcp__recur__edit_plan'), true)
assert.equal(isEditPlanTool('mcp__recur__submit_plan'), false)
assert.equal(editPlanLabel(true), 'Editing plan')
assert.equal(editPlanLabel(false), 'Edited plan')

const plan = parseEditPlan({ plan: 'Add teal paddles.\nKeep scores white.' })
assert.equal(plan, 'Add teal paddles.\nKeep scores white.')
assert.equal(prosePreview(plan), 'Add teal paddles.')
assert.equal(parseEditPlan('{"plan":"nope"}'), '')
assert.ok(!prosePreview(plan).includes('{'))

assert.equal(parseSubmitPlanBody({ body: '  Touch plan.go.  ' }), 'Touch plan.go.')
assert.equal(parseSubmitPlanBody({ summary: 'nope' }), '')
assert.deepEqual(parseSubmitPlanAttachments(['a1', '  ', 'a1', 'shot']), ['a1', 'shot'])
assert.deepEqual(parseSubmitPlanAttachments('nope'), [])

console.log('plantools_test: ok')
