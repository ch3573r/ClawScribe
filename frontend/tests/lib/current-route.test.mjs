import assert from 'node:assert/strict';
import test from 'node:test';
import { loadTsModule } from './load-ts-module.mjs';

const { currentRoute } = loadTsModule('src/lib/current-route.ts');
const at = (pathname, search = '') => JSON.parse(JSON.stringify(currentRoute({ pathname, search })));

test('Home is recognised however the static export names it', () => {
  for (const path of ['/', '', '/index.html', '/index']) assert.equal(at(path).path, '/', path);
});

test('other routes lose trailing slashes and .html', () => {
  assert.equal(at('/settings/').path, '/settings');
  assert.equal(at('/settings.html').path, '/settings');
  assert.equal(at('/meeting-details').path, '/meeting-details');
});

test('the meeting id is read from the query', () => {
  assert.deepEqual(at('/meeting-details', '?id=abc&source=recording'), { path: '/meeting-details', id: 'abc' });
  assert.equal(at('/meeting-details').id, null);
});
