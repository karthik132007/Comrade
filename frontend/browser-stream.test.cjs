const { test } = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs');
const createBrowserStream = new Function(fs.readFileSync('frontend/browser-stream.js', 'utf8').replace('export function', 'function') + '\nreturn createBrowserStream;')();

function harness(startCommand) {
  const calls = [], channels = [], painted = [], ticks = [];
  global.requestAnimationFrame = callback => ticks.push(callback);
  const image = {};
  const stream = createBrowserStream({
    tauri: { Channel: class { constructor() { channels.push(this); } } },
    image, onFrame: frame => painted.push(frame.session_id), onError: error => { throw Error(error); },
    invoke: (command, args) => {
      calls.push({ command, args });
      return command === 'browser_stream_start' && startCommand ? startCommand(args) : Promise.resolve(command === 'browser_stream_start' ? 1 : undefined);
    },
  });
  const frame = id => ({ type: 'frame', stream_id: 1, session_id: id, data_url: `frame-${id}`, width: 640, height: 480 });
  return { calls, channels, painted, ticks, image, stream, frame };
}
test('decode backpressure drops obsolete frames and acknowledges only painted or discarded frames', async () => {
  const h = harness();
  await h.stream.start(640, 480);
  for (let i = 1; i <= 4; i++) h.channels[0].onmessage(h.frame(i));
  assert.equal(h.image.src, 'frame-1');
  assert.deepEqual(h.calls.filter(c => c.command === 'browser_stream_ack').map(c => c.args.sessionId), [2, 3]);
  h.image.onload(); h.ticks.shift()();
  assert.equal(h.image.src, 'frame-4');
  h.image.onload(); h.ticks.shift()();
  assert.deepEqual(h.painted, [1, 4]);
  assert.deepEqual(h.calls.filter(c => c.command === 'browser_stream_ack').map(c => c.args.sessionId), [2, 3, 1, 4]);
  h.stream.stop();
  h.channels[0].onmessage(h.frame(5));
  assert.equal(h.image.src, 'frame-4', 'hidden panes must ignore late frames');
  assert.equal(h.stream.active, false);
});
test('closing before startup finishes stops the returned stream id', async () => {
  let resolve;
  const h = harness(() => new Promise(done => { resolve = done; }));
  const starting = h.stream.start(640, 480);
  h.stream.stop();
  resolve(7);
  assert.equal(await starting, false);
  assert(h.calls.some(c => c.command === 'browser_stream_stop' && c.args.streamId === 7));
  assert.equal(h.stream.active, false);
});
test('resize targets the current stream and never revives a stopped stream', async () => {
  const h = harness();
  await h.stream.start(640, 480);
  await h.stream.resize(800, 600);
  assert.deepEqual(h.calls.find(c => c.command === 'browser_stream_resize').args, { streamId: 1, width: 800, height: 600 });
  h.stream.stop();
  await h.stream.resize(900, 700);
  assert.equal(h.calls.filter(c => c.command === 'browser_stream_resize').length, 1);
});
