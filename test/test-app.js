// Pure-logic tests for frontend/app.js helpers. Run: node test/test-app.js
const assert = require('assert');
const { compareVersions } = require('../frontend/app.js');

assert.ok(compareVersions('9', '10') < 0);
assert.ok(compareVersions('10', '9') > 0);
assert.ok(compareVersions('2.9', '2.28') < 0);
assert.ok(compareVersions('2.28', '2.9') > 0);
assert.ok(compareVersions('18', '18.1') < 0);
assert.ok(compareVersions('18', '18') === 0);
assert.ok(compareVersions('11', '22') < 0);
assert.ok(compareVersions('1.2.3', '1.10.0') < 0);

console.log('test-app.js: all assertions passed');
