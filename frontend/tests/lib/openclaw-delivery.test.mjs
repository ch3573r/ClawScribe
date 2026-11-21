import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { openClawDeliveryLabel } = loadTsModule('src/lib/openclaw-delivery.ts');

test('complete settings do not imply successful delivery', () => {
  const configured = { enabled: true, ready: true, delivery_state: 'unverified' };
  assert.equal(openClawDeliveryLabel(configured).text, 'No confirmed delivery');
  assert.equal(openClawDeliveryLabel({ ...configured, delivery_state: 'failed' }).text, 'Delivery failed');
  assert.equal(openClawDeliveryLabel({ ...configured, delivery_state: 'unknown' }).text, 'Delivery uncertain');
  assert.equal(openClawDeliveryLabel({ ...configured, delivery_state: 'deferred' }).text, 'Handoff needs review');
  assert.equal(openClawDeliveryLabel({ ...configured, delivery_state: 'submitted' }).text, 'Last handoff accepted');
});

test('configuration changes stay visible even after an accepted handoff', () => {
  assert.equal(openClawDeliveryLabel({ enabled: false, ready: false, delivery_state: 'submitted' }).text, 'Off');
  assert.equal(openClawDeliveryLabel({ enabled: true, ready: false, delivery_state: 'submitted' }).text, 'Settings incomplete');
});
