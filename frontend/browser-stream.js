/** Chromium produces frames; the pane acknowledges only decoded frames.
 * At most one image is decoding and one newer image is waiting, so a slow
 * webview cannot accumulate seconds of obsolete frames. */
export function createBrowserStream({ invoke, tauri, image, onFrame, onError }) {
  var supported = !!(tauri && tauri.Channel);
  var active = false;
  var wanted = false;
  var generation = 0;
  var streamId = null;
  var starting = null;
  var painting = null;
  var pending = null;
  var width = 640, height = 640;

  function acknowledge(frame) {
    invoke('browser_stream_ack', { streamId: frame.stream_id, sessionId: frame.session_id }).catch(function () {});
  }
  function paint(frame, version) {
    if (!active || version !== generation) { acknowledge(frame); return; }
    if (painting) {
      if (pending) acknowledge(pending);
      pending = frame;
      return;
    }
    painting = frame;
    image.onload = function () {
      if (version !== generation) return;
      onFrame(frame);
      requestAnimationFrame(function () {
        if (version !== generation) return;
        acknowledge(frame);
        painting = null;
        if (pending) { var next = pending; pending = null; paint(next, version); }
      });
    };
    image.onerror = function () {
      if (version !== generation) return;
      acknowledge(frame);
      painting = null;
      if (pending) { var next = pending; pending = null; paint(next, version); }
    };
    image.src = frame.data_url;
  }
  function start(w, h) {
    if (!supported) return Promise.resolve(false);
    wanted = true;
    width = w; height = h;
    if (starting) return starting.then(function () { return wanted && !active ? start(width, height) : active; });
    if (active) return Promise.resolve(true);
    active = true;
    var version = ++generation;
    var channel = new tauri.Channel();
    channel.onmessage = function (event) {
      if (version !== generation || !active) return;
      if (event.type === 'error') {
        active = false;
        painting = pending = null;
        onError(event.message);
        return;
      }
      if (event.type === 'frame') paint(event, version);
    };
    starting = invoke('browser_stream_start', { width: w, height: h, onFrame: channel }).then(function (id) {
      if (version !== generation || !wanted) {
        return invoke('browser_stream_stop', { streamId: id }).then(function () { return false; });
      }
      streamId = id;
      return true;
    }).catch(function (error) {
      if (version === generation) { active = false; onError(error.message || String(error)); }
      return false;
    }).finally(function () { starting = null; });
    return starting;
  }
  function stop() {
    wanted = active = false;
    generation++;
    painting = pending = null;
    image.onload = image.onerror = null;
    if (streamId != null) invoke('browser_stream_stop', { streamId: streamId }).catch(function () {});
    streamId = null;
  }
  function resize(w, h) {
    width = w; height = h;
    if (active && streamId != null) {
      return invoke('browser_stream_resize', { streamId: streamId, width: w, height: h });
    }
    return Promise.resolve();
  }
  return { start, stop, resize, supported, get active() { return active; } };
}
