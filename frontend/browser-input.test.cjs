const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const createBrowserInputQueue = new Function(fs.readFileSync('frontend/browser-input.js', 'utf8').replace('export function', 'function') + '\nreturn createBrowserInputQueue;')();

test('slow input replies coalesce movement and scrolling without losing clicks or text', async () => {
  const calls = [];
  let release;
  const queue = createBrowserInputQueue((command, args) => {
    calls.push({ command, args });
    return calls.length === 1 ? new Promise(resolve => { release = resolve; }) : Promise.resolve();
  });
  const pointer = (type, values = {}) => queue('browser_pointer', { event: { type, x: 10, y: 10, modifiers: 0, ...values } });
  const pending = [pointer('mousePressed')];
  await Promise.resolve();
  for (let i = 0; i < 100; i++) pending.push(pointer('mouseMoved', { x: i }));
  pending.push(pointer('mouseReleased'));
  for (let i = 0; i < 100; i++) pending.push(pointer('mouseWheel', { deltaX: 0, deltaY: 3 }));
  pending.push(queue('browser_type_text', { text: 'hello' }));
  release();
  await Promise.all(pending);
  assert.equal(calls.length, 5);
  assert.equal(calls[1].args.event.x, 99);
  assert.equal(calls[2].args.event.type, 'mouseReleased');
  assert.equal(calls[3].args.event.deltaY, 300);
  assert.equal(calls[4].args.text, 'hello');
});

test('tab switches discard unsent events and input recovers after a failed command', async () => {
  let reject;
  const calls = [];
  const queue = createBrowserInputQueue(command => {
    calls.push(command);
    return calls.length === 1 ? new Promise((resolve, fail) => { reject = fail; }) : Promise.resolve();
  });
  const first = queue('first', {}).catch(() => {});
  const stale = queue('stale', {});
  await Promise.resolve();
  queue.clear();
  reject(Error('disconnected'));
  await Promise.all([first, stale, queue('new-page', {})]);
  assert.deepEqual(calls, ['first', 'new-page']);
});
