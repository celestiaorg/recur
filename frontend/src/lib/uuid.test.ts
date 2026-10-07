import { expect, test } from 'vitest'
import { uuidv7 } from './uuid'

test('uuidv7 is RFC 9562 version 7', () => {
  const id = uuidv7()
  expect(id).toMatch(/^[0-9a-f]{8}-[0-9a-f]{4}-7[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/)
})
